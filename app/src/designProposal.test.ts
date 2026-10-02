import { describe, expect, it } from "vitest";
import editorJs from "../src-tauri/src/design/editor.js?raw";
import proposalRs from "../src-tauri/src/review_surface/proposal.rs?raw";
import { hostMessage, parseBeacon } from "./designPreview.ts";
import { changeLine, MAX_TOKENS, PROPERTY_ALLOWLIST } from "./designProposal.ts";

const msg = (o: Record<string, unknown>) => ({ source: "skein-design", v: 1, ...o });
const element = {
	selector: "#a",
	tag: "div",
	text: "hi",
	attrs: {},
	rect: { x: 1, y: 2, w: 3, h: 4 },
};
const change = (c: Record<string, unknown>) => parseBeacon(msg({ type: "edit-change", change: c }));

describe("edit-picked", () => {
	const picked = (o: Record<string, unknown> = {}) =>
		msg({
			type: "edit-picked",
			element,
			computed: { "padding-left": "12px", color: "rgb(0, 0, 0)" },
			tokens: [{ name: "--space-4", value: "16px" }],
			...o,
		});

	it("parses a valid beacon", () => {
		const b = parseBeacon(picked());
		expect(b).toMatchObject({
			type: "edit-picked",
			computed: { "padding-left": "12px", color: "rgb(0, 0, 0)" },
			tokens: [{ name: "--space-4", value: "16px" }],
		});
	});
	it("rejects a bad element, computed property or token", () => {
		expect(parseBeacon(picked({ element: { tag: "div" } }))).toBeNull();
		expect(parseBeacon(picked({ computed: { "z-index": "1" } }))).toBeNull();
		expect(parseBeacon(picked({ computed: { color: "x".repeat(201) } }))).toBeNull();
		expect(parseBeacon(picked({ tokens: [{ name: "space", value: "1" }] }))).toBeNull();
		expect(parseBeacon(picked({ tokens: "no" }))).toBeNull();
	});
	it("caps tokens at 200", () => {
		const t = (n: number) => Array.from({ length: n }, (_, i) => ({ name: `--t${i}`, value: "1" }));
		expect(parseBeacon(picked({ tokens: t(MAX_TOKENS) }))).not.toBeNull();
		expect(parseBeacon(picked({ tokens: t(MAX_TOKENS + 1) }))).toBeNull();
	});
});

describe("edit-change", () => {
	it("parses each kind", () => {
		expect(
			change({ kind: "style", property: "gap", from: "4px", to: "8px", token: "--gap" }),
		).toEqual({
			type: "edit-change",
			change: { kind: "style", property: "gap", from: "4px", to: "8px", token: "--gap" },
		});
		expect(change({ kind: "style", property: "opacity", from: "1", to: "0.5" })).toEqual({
			type: "edit-change",
			change: { kind: "style", property: "opacity", from: "1", to: "0.5" },
		});
		expect(change({ kind: "offset", dx: 4, dy: -8 })).toEqual({
			type: "edit-change",
			change: { kind: "offset", dx: 4, dy: -8 },
		});
		expect(change({ kind: "text", from: "Save", to: "Save changes" })).toEqual({
			type: "edit-change",
			change: { kind: "text", from: "Save", to: "Save changes" },
		});
	});
	it("rejects properties outside the allowlist", () => {
		expect(change({ kind: "style", property: "position", from: "a", to: "b" })).toBeNull();
		expect(change({ kind: "style", property: "__proto__", from: "a", to: "b" })).toBeNull();
		expect(change({ kind: "style", from: "a", to: "b" })).toBeNull();
	});
	it("rejects oversize, control-char and malformed values", () => {
		const ok = { kind: "style", property: "color" };
		expect(change({ ...ok, from: "a", to: "x".repeat(201) })).toBeNull();
		expect(change({ ...ok, from: "a\nb", to: "c" })).toBeNull();
		expect(change({ ...ok, from: "a", to: 5 })).toBeNull();
		expect(change({ ...ok, from: "a", to: "b", token: "space-4" })).toBeNull();
		expect(change({ ...ok, from: "a", to: "b", token: 1 })).toBeNull();
		expect(change({ kind: "text", from: "a", to: "x".repeat(2001) })).toBeNull();
		expect(change({ kind: "text", from: "a", to: `b${String.fromCharCode(0)}` })).toBeNull();
	});
	it("rejects non-finite or huge offsets", () => {
		expect(change({ kind: "offset", dx: Number.NaN, dy: 0 })).toBeNull();
		expect(change({ kind: "offset", dx: Number.POSITIVE_INFINITY, dy: 0 })).toBeNull();
		expect(change({ kind: "offset", dx: 100001, dy: 0 })).toBeNull();
		expect(change({ kind: "offset", dx: "4", dy: 0 })).toBeNull();
	});
	it("rejects unknown kinds and a missing change", () => {
		expect(change({ kind: "html", to: "<b>" })).toBeNull();
		expect(parseBeacon(msg({ type: "edit-change" }))).toBeNull();
		expect(parseBeacon(msg({ type: "edit-change", change: "x" }))).toBeNull();
	});
	it("ignores the wrong envelope", () => {
		const b = { type: "edit-change", change: { kind: "offset", dx: 1, dy: 1 } };
		expect(parseBeacon({ ...b, source: "evil", v: 1 })).toBeNull();
		expect(parseBeacon({ ...b, source: "skein-design", v: 2 })).toBeNull();
	});
});

describe("edit-cancelled and host messages", () => {
	it("parses edit-cancelled", () => {
		expect(parseBeacon(msg({ type: "edit-cancelled" }))).toEqual({ type: "edit-cancelled" });
	});
	it("wraps the new host messages in the envelope", () => {
		expect(hostMessage({ type: "edit-start" })).toEqual({
			source: "skein-host",
			v: 1,
			type: "edit-start",
		});
		expect(hostMessage({ type: "edit-set", property: "gap", value: "4px" }).v).toBe(1);
		expect(hostMessage({ type: "edit-end", revert: true })).toMatchObject({ revert: true });
		expect(hostMessage({ type: "proposals", items: [] })).toMatchObject({ items: [] });
	});
	it("keeps the allowlist at 23 unique properties", () => {
		expect(new Set(PROPERTY_ALLOWLIST).size).toBe(23);
	});
});

describe("changeLine", () => {
	it("words each kind", () => {
		expect(
			changeLine({
				kind: "style",
				property: "padding-left",
				from: "12px",
				to: "16px",
				token: "--space-4",
			}),
		).toBe("padding-left 12px → 16px (token --space-4)");
		expect(changeLine({ kind: "style", property: "gap", from: "4px", to: "8px" })).toBe(
			"gap 4px → 8px",
		);
		expect(changeLine({ kind: "offset", dx: 4, dy: -2 })).toBe("move 4px, -2px");
		expect(changeLine({ kind: "text", from: "Save", to: "Go" })).toBe('text "Save" → "Go"');
	});

	it("clips untrusted strings", () => {
		const long = "x".repeat(500);
		const line = changeLine({ kind: "text", from: long, to: "b" });
		expect(line.length).toBeLessThan(150);
		expect(line).toContain("…");
	});
});

describe("allowlist copies", () => {
	const quoted = (block: string) => [...block.matchAll(/"([a-z-]+)"/g)].map((m) => m[1]);
	const entriesAfter = (src: string, marker: string) => {
		const open = src.indexOf("[", src.indexOf(marker) + marker.length);
		return quoted(src.slice(open, src.indexOf("]", open)));
	};

	it("matches the Rust and iframe copies", () => {
		expect(entriesAfter(proposalRs, "PROPERTY_ALLOWLIST: &[&str] =")).toEqual([
			...PROPERTY_ALLOWLIST,
		]);
		expect(entriesAfter(editorJs, "const ALLOWLIST")).toEqual([...PROPERTY_ALLOWLIST]);
	});
});
