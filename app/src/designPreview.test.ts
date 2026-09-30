import { describe, expect, it } from "vitest";
import {
	type Beacon,
	MAX_BEACONS,
	MAX_BEACON_TEXT,
	parseBeacon,
	previewUrl,
	pushBeacon,
} from "./designPreview.ts";

const msg = (o: Record<string, unknown>) => ({ source: "skein-design", v: 1, ...o });

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
