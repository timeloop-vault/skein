import { describe, expect, it } from "vitest";
import {
	type Beacon,
	MAX_BEACONS,
	MAX_BEACON_TEXT,
	hostMessage,
	parseBeacon,
	previewUrl,
	pushBeacon,
	retryDelay,
	stripSourcePrefix,
} from "./designPreview.ts";

const msg = (o: Record<string, unknown>) => ({ source: "skein-design", v: 1, ...o });

const rect = { x: 1, y: 2, w: 3, h: 4 };
const el = (o: Record<string, unknown> = {}) => ({
	selector: "#a > div:nth-of-type(2)",
	tag: "div",
	text: "hi",
	attrs: { class: "x" },
	rect,
	...o,
});

describe("stripSourcePrefix", () => {
	it("maps preview URLs to worktree paths", () => {
		expect(stripSourcePrefix("/http:/127.0.0.1:1234/preview/tok/proto/shell.jsx")).toBe(
			"proto/shell.jsx",
		);
		expect(stripSourcePrefix("http://127.0.0.1:1/preview/tok/a/b/c.html?v=2")).toBe("a/b/c.html");
		expect(stripSourcePrefix("/preview/tok/my%20dir/f%C3%A4.jsx")).toBe("my dir/fä.jsx");
	});
	it("refuses traversal, backslashes, empties and non-preview names", () => {
		expect(stripSourcePrefix("/preview/tok/../x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a/%2e%2e/x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%5Cb.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%2Fb.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/C%3A/x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok")).toBeUndefined();
		expect(stripSourcePrefix("/preview//x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a//b.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/%E0%A4%A.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a/b:c.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%3Ab.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%00b.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/My%20Proto.jsx")).toBe("My Proto.jsx");
		expect(stripSourcePrefix(`/preview/tok/${"a".repeat(501)}`)).toBeUndefined();
		expect(stripSourcePrefix(`/preview/tok/${"a".repeat(500)}`)).toBe("a".repeat(500));
		expect(stripSourcePrefix("/Inline Babel script")).toBeUndefined();
		expect(stripSourcePrefix("http://cdn.example/react.js")).toBeUndefined();
	});
});

describe("parseBeacon element messages", () => {
	it("parses picked, converting rawSource to a worktree source", () => {
		const b = parseBeacon(
			msg({
				type: "picked",
				element: el({
					odId: "hero",
					rawSource: {
						fileName: "/http:/127.0.0.1:1234/preview/tok/proto/shell.jsx",
						lineNumber: 7,
						columnNumber: 3,
					},
				}),
			}),
		);
		expect(b).toEqual({
			type: "picked",
			element: { ...el(), odId: "hero", source: { file: "proto/shell.jsx", line: 7, column: 3 } },
		});
	});
	it("drops a source that is not a preview file", () => {
		const b = parseBeacon(
			msg({
				type: "picked",
				element: el({ rawSource: { fileName: "/Inline Babel script", lineNumber: 1 } }),
			}),
		);
		expect(b).toEqual({ type: "picked", element: el() });
	});
	it("rejects malformed picked", () => {
		expect(parseBeacon(msg({ type: "picked" }))).toBeNull();
		expect(parseBeacon(msg({ type: "picked", element: el({ selector: 3 }) }))).toBeNull();
		expect(parseBeacon(msg({ type: "picked", element: el({ rect: { x: 1 } }) }))).toBeNull();
		expect(
			parseBeacon(msg({ type: "picked", element: el({ rect: { ...rect, x: Number.NaN } }) })),
		).toBeNull();
	});
	it("clips long fields and caps attrs", () => {
		const attrs: Record<string, string> = {};
		for (let i = 0; i < 40; i++) attrs[`data-${i}`] = "v".repeat(1000);
		attrs["k".repeat(200)] = "x";
		const b = parseBeacon(
			msg({
				type: "picked",
				element: el({
					selector: "s".repeat(5000),
					tag: "t".repeat(500),
					text: "x".repeat(5000),
					odId: "o".repeat(1000),
					attrs: { ...attrs, bad: 5 },
				}),
			}),
		);
		if (b?.type !== "picked") throw new Error("expected picked");
		expect(b.element.selector).toHaveLength(1000);
		expect(b.element.tag).toHaveLength(64);
		expect(b.element.text).toHaveLength(MAX_BEACON_TEXT);
		expect(b.element.odId).toHaveLength(200);
		expect(Object.keys(b.element.attrs)).toHaveLength(16);
		expect(Object.values(b.element.attrs).every((v) => v.length <= 300)).toBe(true);
	});
	it("parses pick-cancelled", () => {
		expect(parseBeacon(msg({ type: "pick-cancelled" }))).toEqual({ type: "pick-cancelled" });
	});
	it("parses located, dropping malformed items and capping arrays", () => {
		const found = {
			bySelector: el(),
			byOdId: Array.from({ length: 30 }, () => el()),
			byText: [el(), { nope: 1 }],
			sameTag: Array.from({ length: 300 }, () => el()),
		};
		const b = parseBeacon(
			msg({
				type: "located",
				requestId: "r1",
				results: [
					{ id: "a", found },
					{ id: "b", found: { bySelector: null, byOdId: [], byText: [], sameTag: [] } },
					{ id: "c" },
					"junk",
					{ found },
				],
				files: [
					"http://h/preview/tok/a.html",
					"/Inline Babel script",
					5,
					"http://h/preview/tok/../x",
				],
			}),
		);
		if (b?.type !== "located") throw new Error("expected located");
		expect(b.requestId).toBe("r1");
		expect(b.results.map((r) => r.id)).toEqual(["a", "b"]);
		expect(b.results[0]?.found.byOdId).toHaveLength(10);
		expect(b.results[0]?.found.byText).toHaveLength(1);
		expect(b.results[0]?.found.sameTag).toHaveLength(200);
		expect(b.results[1]?.found.bySelector).toBeNull();
		expect(b.files).toEqual(["a.html"]);
	});
	it("caps located results at 100 and files at 200", () => {
		const empty = { bySelector: null, byOdId: [], byText: [], sameTag: [] };
		const b = parseBeacon(
			msg({
				type: "located",
				requestId: "r",
				results: Array.from({ length: 150 }, (_, i) => ({ id: `i${i}`, found: empty })),
				files: Array.from({ length: 400 }, (_, i) => `/preview/t/f${i}.js`),
			}),
		);
		if (b?.type !== "located") throw new Error("expected located");
		expect(b.results).toHaveLength(100);
		expect(b.files).toHaveLength(200);
	});
	it("rejects located without requestId or results", () => {
		expect(parseBeacon(msg({ type: "located", results: [] }))).toBeNull();
		expect(parseBeacon(msg({ type: "located", requestId: "r" }))).toBeNull();
	});
});

describe("hostMessage", () => {
	it("adds the envelope", () => {
		expect(hostMessage({ type: "highlight", n: 2 })).toEqual({
			source: "skein-host",
			v: 1,
			type: "highlight",
			n: 2,
		});
	});
});

describe("parseBeacon", () => {
	it("accepts the three v1 shapes", () => {
		expect(parseBeacon(msg({ type: "ready", href: "http://x/" }))).toEqual({
			type: "ready",
			href: "http://x/",
		});
		expect(parseBeacon(msg({ type: "resource-error", tag: "img", url: "a.png" }))).toEqual({
			type: "resource-error",
			tag: "img",
			url: "a.png",
		});
		expect(
			parseBeacon(msg({ type: "script-error", message: "boom", url: "a.js", line: 3 })),
		).toEqual({ type: "script-error", message: "boom", url: "a.js", line: 3 });
		expect(parseBeacon(msg({ type: "script-error", message: "boom" }))).toEqual({
			type: "script-error",
			message: "boom",
		});
	});

	it("rejects a wrong source, version, type or non-object", () => {
		expect(parseBeacon({ ...msg({ type: "ready", href: "x" }), source: "other" })).toBeNull();
		expect(parseBeacon({ ...msg({ type: "ready", href: "x" }), v: 2 })).toBeNull();
		expect(parseBeacon(msg({ type: "nope" }))).toBeNull();
		expect(parseBeacon(msg({}))).toBeNull();
		expect(parseBeacon(null)).toBeNull();
		expect(parseBeacon("skein-design")).toBeNull();
	});

	it("rejects wrongly typed fields", () => {
		expect(parseBeacon(msg({ type: "ready", href: 5 }))).toBeNull();
		expect(parseBeacon(msg({ type: "resource-error", tag: "img" }))).toBeNull();
		expect(parseBeacon(msg({ type: "script-error", message: {} }))).toBeNull();
	});

	it("drops a non-numeric line and truncates long strings", () => {
		const long = "x".repeat(MAX_BEACON_TEXT * 3);
		const b = parseBeacon(msg({ type: "script-error", message: long, url: long, line: "7" }));
		expect(b).toEqual({
			type: "script-error",
			message: "x".repeat(MAX_BEACON_TEXT),
			url: "x".repeat(MAX_BEACON_TEXT),
		});
	});
});

describe("pushBeacon", () => {
	it("caps the list at MAX_BEACONS", () => {
		const b: Beacon = { type: "ready", href: "h" };
		let list: Beacon[] = [];
		for (let i = 0; i < MAX_BEACONS + 10; i++) list = pushBeacon(list, b);
		expect(list).toHaveLength(MAX_BEACONS);
	});
});

describe("previewUrl", () => {
	const base = "http://127.0.0.1:1/preview/tok/";
	it("builds a nested path with ?v=", () => {
		expect(previewUrl(base, "a/b/index.html", 2)).toBe(`${base}a/b/index.html?v=2`);
	});
	it("encodes spaces and unicode per segment, keeping the slashes", () => {
		expect(previewUrl(base, "my mocks/åäö #1.html", 0)).toBe(
			`${base}my%20mocks/%C3%A5%C3%A4%C3%B6%20%231.html?v=0`,
		);
	});
});

describe("retryDelay", () => {
	it("doubles from 150 ms, caps at 2 s, then gives up", () => {
		const seq: number[] = [];
		for (let n = 1; ; n++) {
			const d = retryDelay(n);
			if (d === null) break;
			seq.push(d);
		}
		expect(seq.slice(0, 6)).toEqual([150, 300, 600, 1200, 2000, 2000]);
		expect(Math.max(...seq)).toBe(2000);
		const total = seq.reduce((a, b) => a + b, 0);
		expect(total).toBeGreaterThanOrEqual(15000);
		expect(total).toBeLessThan(19000);
	});
});
