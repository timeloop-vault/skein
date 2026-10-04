// designPreview — the pure half of the `design` harness (#433): the
// preview URL and the beacon messages the injected script posts from
// inside the iframe. The page is arbitrary worktree code, so every
// message is UNTRUSTED input: the caller must already have checked
// `e.source === iframe.contentWindow`; this module checks the shape,
// caps every string and never yields anything but plain text.

import { type Change, type EditBeacon, parseEditBeacon } from "./designProposal";
import type { ElementDescriptor, ElementRect, ElementSource, LocateResult } from "./elementAnchor";

export const MAX_BEACON_TEXT = 500;
export const MAX_BEACONS = 50;
/** Same cap as Rust `check_rel_path`. */
const MAX_SOURCE_PATH = 500;
const NUL = String.fromCharCode(0);

export type Beacon =
	| { type: "ready"; href: string }
	| { type: "resource-error"; tag: string; url: string }
	| { type: "script-error"; message: string; url?: string; line?: number }
	| { type: "picked"; element: ElementDescriptor }
	| { type: "pick-cancelled" }
	| { type: "dom-changed" }
	| {
			type: "located";
			requestId: string;
			results: { id: string; found: LocateResult }[];
			/** Worktree-relative paths of the files the page loaded. */
			files: string[];
	  }
	| {
			type: "shownElement";
			requestId: string;
			count: number;
			/** Set only for a unique match. */
			element: ElementDescriptor | null;
			/** The selector was not valid CSS. */
			invalid?: boolean;
	  }
	| {
			type: "invoked";
			requestId: string;
			count: number;
			element: ElementDescriptor | null;
			invalid?: boolean;
			busy?: boolean;
			notVisible?: boolean;
			visible?: boolean;
			domChanged?: boolean;
	  }
	| { type: "shownCleared" }
	/** show_changes (#547): the rendered source sites, one per (fileName, line). */
	| {
			type: "sources";
			requestId: string;
			sites: { fileName: string; line: number; endLine: number; count: number; onScreen: number }[];
			/** The frame stopped listing at its own site cap. */
			capped?: true;
	  }
	| { type: "shownChanges"; requestId: string; highlighted: number; capped?: true }
	| { type: "changesCleared" }
	| { type: "scroll"; x: number; y: number }
	| EditBeacon;

const MAX_ATTRS = 16;
const MAX_ANCHORS = 100;
const MAX_FILES = 200;
const MAX_SITES = 5000;
const MAX_BY_ID = 10;
const MAX_BY_TEXT = 10;
const MAX_SAME_TAG = 200;

const clipTo = (s: string, n: number): string => (s.length > n ? s.slice(0, n) : s);
const cap = (s: string): string => clipTo(s, MAX_BEACON_TEXT);
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

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

const parseDescriptor = (v: unknown): ElementDescriptor | null => {
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
	return out;
};

const parseDescriptors = (v: unknown, max: number): ElementDescriptor[] => {
	if (!Array.isArray(v)) return [];
	const out: ElementDescriptor[] = [];
	for (const item of v.slice(0, max)) {
		const d = parseDescriptor(item);
		if (d) out.push(d);
	}
	return out;
};

const parseLocate = (v: unknown): LocateResult | null => {
	if (!isObj(v)) return null;
	return {
		bySelector: v.bySelector == null ? null : parseDescriptor(v.bySelector),
		byOdId: parseDescriptors(v.byOdId, MAX_BY_ID),
		byText: parseDescriptors(v.byText, MAX_BY_TEXT),
		sameTag: parseDescriptors(v.sameTag, MAX_SAME_TAG),
	};
};

const idOf = (v: unknown): string | null =>
	typeof v === "string" ? clipTo(v, 200) : num(v) !== null ? String(v) : null;

const parseLocated = (d: Record<string, unknown>): Beacon | null => {
	const requestId = idOf(d.requestId);
	if (requestId === null || !Array.isArray(d.results)) return null;
	const results: { id: string; found: LocateResult }[] = [];
	for (const r of d.results.slice(0, MAX_ANCHORS)) {
		if (!isObj(r)) continue;
		const id = idOf(r.id);
		const found = parseLocate(r.found);
		if (id !== null && found) results.push({ id, found });
	}
	const files: string[] = [];
	if (Array.isArray(d.files)) {
		for (const f of d.files.slice(0, MAX_FILES)) {
			const p = typeof f === "string" ? stripSourcePrefix(f) : undefined;
			if (p !== undefined) files.push(p);
		}
	}
	return { type: "located", requestId, results, files };
};

/** Host to iframe messages, handled by the injected picker script. */
export type HostMessage =
	| { type: "pick-start" }
	| { type: "pick-cancel" }
	| {
			type: "locate";
			requestId: string;
			anchors: { id: string; odId?: string; selector: string; tag: string; text: string }[];
	  }
	| { type: "pins"; pins: { n: number; state: string; rect: ElementRect }[] }
	| { type: "highlight"; n: number }
	/** Agent show_element (#512): outline + scroll to a selector's element. */
	| { type: "showElement"; requestId: string; selector: string }
	/** Agent invoke_element (#549): tap or swipe the element at a selector. */
	| {
			type: "invoke";
			requestId: string;
			selector: string;
			action: "tap" | "swipe";
			direction?: string;
			distance?: number;
			pointerType: "mouse" | "touch";
	  }
	/** show_changes (#547): ask for the rendered source sites. */
	| { type: "listSources"; requestId: string }
	/** show_changes (#547): outline every element rendered from these sites. */
	| {
			type: "showChanges";
			requestId: string;
			sites: { fileName: string; line: number; endLine: number }[];
			max: number;
	  }
	| { type: "edit-start" }
	| { type: "edit-set"; property: string; value: string }
	| { type: "edit-end"; revert: boolean }
	| {
			type: "proposals";
			items: {
				id: string;
				anchor: { id: string; odId?: string; selector: string; tag: string; text: string };
				changes: Change[];
			}[];
	  };

/** Wrap a host message in the envelope the injected script checks. */
export const hostMessage = (msg: HostMessage): HostMessage & { source: "skein-host"; v: 1 } => ({
	source: "skein-host",
	v: 1,
	...msg,
});

/** Validate one `message` event payload from the preview iframe.
 *  Returns null for anything that is not a well-formed v1 beacon. */
export const parseBeacon = (data: unknown): Beacon | null => {
	if (typeof data !== "object" || data === null) return null;
	const d = data as Record<string, unknown>;
	if (d.source !== "skein-design" || d.v !== 1) return null;
	switch (d.type) {
		case "ready":
			return typeof d.href === "string" ? { type: "ready", href: cap(d.href) } : null;
		case "resource-error":
			return typeof d.tag === "string" && typeof d.url === "string"
				? { type: "resource-error", tag: cap(d.tag), url: cap(d.url) }
				: null;
		case "script-error": {
			if (typeof d.message !== "string") return null;
			const out: Beacon = { type: "script-error", message: cap(d.message) };
			if (typeof d.url === "string") out.url = cap(d.url);
			if (typeof d.line === "number" && Number.isFinite(d.line)) out.line = d.line;
			return out;
		}
		case "picked": {
			const element = parseDescriptor(d.element);
			return element ? { type: "picked", element } : null;
		}
		case "pick-cancelled":
			return { type: "pick-cancelled" };
		case "dom-changed":
			return { type: "dom-changed" };
		case "located":
			return parseLocated(d);
		case "shownElement": {
			const requestId = idOf(d.requestId);
			const count = num(d.count);
			if (requestId === null || count === null) return null;
			return {
				type: "shownElement",
				requestId,
				count,
				element: parseDescriptor(d.element),
				...(d.invalid === true ? { invalid: true } : {}),
			};
		}
		case "invoked": {
			const requestId = idOf(d.requestId);
			const count = num(d.count);
			if (requestId === null || count === null) return null;
			return {
				type: "invoked",
				requestId,
				count,
				element: parseDescriptor(d.element),
				...(d.invalid === true ? { invalid: true } : {}),
				...(d.busy === true ? { busy: true } : {}),
				...(d.notVisible === true ? { notVisible: true } : {}),
				...(typeof d.visible === "boolean" ? { visible: d.visible } : {}),
				...(typeof d.domChanged === "boolean" ? { domChanged: d.domChanged } : {}),
			};
		}
		case "shownCleared":
			return { type: "shownCleared" };
		case "sources": {
			const requestId = idOf(d.requestId);
			if (requestId === null || !Array.isArray(d.sites)) return null;
			const sites: Extract<Beacon, { type: "sources" }>["sites"] = [];
			for (const s of d.sites.slice(0, MAX_SITES)) {
				if (!isObj(s) || typeof s.fileName !== "string") continue;
				const line = num(s.line);
				const count = num(s.count);
				if (line === null || count === null) continue;
				sites.push({
					fileName: clipTo(s.fileName, 1000),
					line,
					endLine: num(s.endLine) ?? line,
					count,
					onScreen: num(s.onScreen) ?? 0,
				});
			}
			return {
				type: "sources",
				requestId,
				sites,
				...(d.capped === true || d.sites.length > MAX_SITES ? { capped: true as const } : {}),
			};
		}
		case "shownChanges": {
			const requestId = idOf(d.requestId);
			const highlighted = num(d.highlighted);
			if (requestId === null || highlighted === null) return null;
			return {
				type: "shownChanges",
				requestId,
				highlighted,
				...(d.capped === true ? { capped: true as const } : {}),
			};
		}
		case "changesCleared":
			return { type: "changesCleared" };
		case "scroll": {
			const x = num(d.x);
			const y = num(d.y);
			return x === null || y === null ? null : { type: "scroll", x, y };
		}
		case "edit-picked":
		case "edit-change":
		case "edit-cancelled":
			return parseEditBeacon(d, parseDescriptor);
		default:
			return null;
	}
};

/** Append a beacon, keeping at most MAX_BEACONS (the oldest win). */
export const pushBeacon = (list: readonly Beacon[], b: Beacon): Beacon[] =>
	list.length >= MAX_BEACONS ? [...list] : [...list, b];

/** `base` (ends with `/`) + the worktree-relative `entry`, each path
 *  segment percent-encoded, plus a `?v=` cache-buster and any extra
 *  query `params` (URI-encoded, appended after `v`). */
export const previewUrl = (
	base: string,
	entry: string,
	version: number,
	params: [string, string][] = [],
): string =>
	`${base}${entry.split("/").map(encodeURIComponent).join("/")}?v=${version}${params
		.map(([k, v]) => `&${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
		.join("")}`;

export const RETRY_FIRST_MS = 150;
export const RETRY_MAX_MS = 2000;
export const RETRY_BUDGET_MS = 15000;

/** Delay before retry number `failures` (1 = after the first failure):
 *  150 ms doubling, capped at 2 s. Null once the delays already spent
 *  reach the ~15 s budget — the caller gives up and shows the error. */
export const retryDelay = (failures: number): number | null => {
	let spent = 0;
	let delay = RETRY_FIRST_MS;
	for (let i = 1; i < failures; i++) {
		spent += delay;
		delay = Math.min(delay * 2, RETRY_MAX_MS);
	}
	return spent >= RETRY_BUDGET_MS ? null : delay;
};
