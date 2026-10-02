// designProposal — the wire shape of a proposed edit (#436) and the
// defensive parsers for the edit beacons the iframe posts. The iframe is
// arbitrary worktree code, so everything here is UNTRUSTED input: unknown
// properties, oversize strings and non-finite numbers drop the whole
// beacon (null, never a throw). Rust re-validates authoritatively.

import type { ElementDescriptor } from "./elementAnchor";

export type Change =
	| { kind: "style"; property: string; from: string; to: string; token?: string }
	| { kind: "offset"; dx: number; dy: number }
	| { kind: "text"; from: string; to: string };

export type Proposal = { changes: Change[] };

/** Keep identical to PROPERTY_ALLOWLIST in src-tauri/src/review_surface/proposal.rs
 *  and ALLOWLIST in src-tauri/src/design/editor.js (a test checks all three). */
export const PROPERTY_ALLOWLIST: readonly string[] = [
	"width",
	"height",
	"margin-top",
	"margin-right",
	"margin-bottom",
	"margin-left",
	"padding-top",
	"padding-right",
	"padding-bottom",
	"padding-left",
	"gap",
	"row-gap",
	"column-gap",
	"color",
	"background-color",
	"border-color",
	"border-radius",
	"font-family",
	"font-size",
	"font-weight",
	"line-height",
	"letter-spacing",
	"opacity",
];

export const MAX_STYLE_VALUE = 200;
export const MAX_TEXT_VALUE = 2000;
export const MAX_TOKENS = 200;
export const MAX_OFFSET = 100000;
const TOKEN_NAME = /^--[A-Za-z0-9_-]{1,100}$/;

export type DesignToken = { name: string; value: string };

/** A design-pane edit change, plus the pre-edit state of a picked element. */
export type EditBeacon =
	| {
			type: "edit-picked";
			element: ElementDescriptor;
			computed: Record<string, string>;
			tokens: DesignToken[];
	  }
	| { type: "edit-change"; change: Change }
	| { type: "edit-cancelled" };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

const hasControl = (s: string): boolean => {
	for (let i = 0; i < s.length; i++) {
		const c = s.charCodeAt(i);
		if (c < 32 || c === 127) return true;
	}
	return false;
};

const styleValue = (v: unknown): string | null =>
	typeof v === "string" && v.length <= MAX_STYLE_VALUE && !hasControl(v) ? v : null;

const textValue = (v: unknown): string | null =>
	typeof v === "string" && v.length <= MAX_TEXT_VALUE && !v.includes(String.fromCharCode(0))
		? v
		: null;

const offsetValue = (v: unknown): number | null =>
	typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= MAX_OFFSET ? v : null;

/** One change from the iframe, or null if it is not well formed. */
export const parseChange = (v: unknown): Change | null => {
	if (!isObj(v)) return null;
	if (v.kind === "style") {
		const from = styleValue(v.from);
		const to = styleValue(v.to);
		if (typeof v.property !== "string" || !PROPERTY_ALLOWLIST.includes(v.property)) return null;
		if (from === null || to === null) return null;
		const out: Change = { kind: "style", property: v.property, from, to };
		if (v.token !== undefined) {
			if (typeof v.token !== "string" || !TOKEN_NAME.test(v.token)) return null;
			out.token = v.token;
		}
		return out;
	}
	if (v.kind === "offset") {
		const dx = offsetValue(v.dx);
		const dy = offsetValue(v.dy);
		return dx === null || dy === null ? null : { kind: "offset", dx, dy };
	}
	if (v.kind === "text") {
		const from = textValue(v.from);
		const to = textValue(v.to);
		return from === null || to === null ? null : { kind: "text", from, to };
	}
	return null;
};

const clipText = (s: string, max: number): string => (s.length > max ? `${s.slice(0, max)}…` : s);

/** One plain-text line describing a change, for the review pane. Values
 *  came from page content, so each is clipped to `max` chars. */
export const changeLine = (c: Change, max = 120): string => {
	switch (c.kind) {
		case "style": {
			const token = c.token ? ` (token ${clipText(c.token, max)})` : "";
			return `${c.property} ${clipText(c.from, max)} → ${clipText(c.to, max)}${token}`;
		}
		case "offset":
			return `move ${c.dx}px, ${c.dy}px`;
		case "text":
			return `text "${clipText(c.from, max)}" → "${clipText(c.to, max)}"`;
	}
};

const parseComputed = (v: unknown): Record<string, string> | null => {
	if (!isObj(v)) return null;
	const out: Record<string, string> = {};
	for (const [k, val] of Object.entries(v)) {
		if (!PROPERTY_ALLOWLIST.includes(k)) return null;
		const s = styleValue(val);
		if (s === null) return null;
		out[k] = s;
	}
	return out;
};

const parseTokens = (v: unknown): DesignToken[] | null => {
	if (!Array.isArray(v) || v.length > MAX_TOKENS) return null;
	const out: DesignToken[] = [];
	for (const t of v) {
		if (!isObj(t) || typeof t.name !== "string" || !TOKEN_NAME.test(t.name)) return null;
		const value = styleValue(t.value);
		if (value === null) return null;
		out.push({ name: t.name, value });
	}
	return out;
};

/** The edit beacons (`edit-picked`, `edit-change`, `edit-cancelled`);
 *  `descriptor` is designPreview's element parser, passed in to avoid an
 *  import cycle. Null for anything else or anything malformed. */
export const parseEditBeacon = (
	d: Record<string, unknown>,
	descriptor: (v: unknown) => ElementDescriptor | null,
): EditBeacon | null => {
	switch (d.type) {
		case "edit-picked": {
			const element = descriptor(d.element);
			const computed = parseComputed(d.computed);
			const tokens = parseTokens(d.tokens);
			return element && computed && tokens
				? { type: "edit-picked", element, computed, tokens }
				: null;
		}
		case "edit-change": {
			const change = parseChange(d.change);
			return change ? { type: "edit-change", change } : null;
		}
		case "edit-cancelled":
			return { type: "edit-cancelled" };
		default:
			return null;
	}
};
