import { describe, expect, it } from "vitest";
import type { ShowElementResult } from "./designControl.ts";
import {
	BUSY_ERROR,
	type InvokedBeacon,
	invokeAnchorResult,
	invokeSelectorResult,
	parseInvokeElementArgs,
} from "./designInvoke.ts";
import type { ElementDescriptor } from "./elementAnchor.ts";

const el: ElementDescriptor = {
	selector: "div > p",
	tag: "p",
	text: "hi",
	attrs: {},
	rect: { x: 1, y: 2, w: 3, h: 4 },
};
const base = { roomId: "r", harnessId: "h" };
const beacon = (b: Partial<InvokedBeacon>): InvokedBeacon => ({
	type: "invoked",
	requestId: "agent-inv-1",
	count: 1,
	element: el,
	...b,
});

describe("parseInvokeElementArgs", () => {
	it("defaults to a tap on a selector", () => {
		expect(parseInvokeElementArgs({ ...base, selector: "p" })).toEqual({
			ok: true,
			value: { ...base, selector: "p", action: "tap" },
		});
	});
	it("defaults swipe distance and keeps direction", () => {
		const r = parseInvokeElementArgs({ ...base, selector: "p", action: "swipe", direction: "up" });
		expect(r).toEqual({
			ok: true,
			value: { ...base, selector: "p", action: "swipe", direction: "up", distance: 120 },
		});
	});
	it("accepts an anchor and null for omitted fields", () => {
		const r = parseInvokeElementArgs({
			...base,
			selector: null,
			anchor: { tag: "p" },
			action: null,
			direction: null,
		});
		expect(r.ok && r.value.anchor?.tag).toBe("p");
	});
	it.each([
		["neither target", {}],
		["both targets", { selector: "p", anchor: { tag: "p" } }],
		["empty selector", { selector: "" }],
		["bad anchor", { anchor: { text: "x" } }],
		["bad action", { selector: "p", action: "drag" }],
		["tap with direction", { selector: "p", direction: "left" }],
		["tap with distance", { selector: "p", distance: 50 }],
		["swipe without direction", { selector: "p", action: "swipe" }],
		["swipe bad direction", { selector: "p", action: "swipe", direction: "diagonal" }],
		["distance too small", { selector: "p", action: "swipe", direction: "up", distance: 7 }],
		["distance too big", { selector: "p", action: "swipe", direction: "up", distance: 2001 }],
		["distance fractional", { selector: "p", action: "swipe", direction: "up", distance: 9.5 }],
		["distance string", { selector: "p", action: "swipe", direction: "up", distance: "50" }],
	])("refuses %s", (_n, extra) => {
		const r = parseInvokeElementArgs({ ...base, ...extra });
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.error.startsWith("bad_arguments:")).toBe(true);
	});
	it("accepts the distance bounds", () => {
		for (const distance of [8, 2000]) {
			const r = parseInvokeElementArgs({
				...base,
				selector: "p",
				action: "swipe",
				direction: "left",
				distance,
			});
			expect(r.ok).toBe(true);
		}
	});
	it("requires room and harness", () => {
		expect(parseInvokeElementArgs({ selector: "p" }).ok).toBe(false);
	});
});

describe("invokeSelectorResult", () => {
	it("invokes a single match and passes domChanged through", () => {
		expect(invokeSelectorResult(beacon({ domChanged: true }), "tap")).toEqual({
			tier: "selector",
			invoked: true,
			action: "tap",
			element: el,
			count: 1,
			visible: false,
			domChanged: true,
		});
		expect(invokeSelectorResult(beacon({}), "swipe").domChanged).toBe(false);
	});
	it("refuses an ambiguous selector", () => {
		expect(invokeSelectorResult(beacon({ count: 3, element: null }), "tap")).toEqual({
			tier: "ambiguous",
			invoked: false,
			action: "tap",
			element: null,
			count: 3,
		});
	});
	it("reports no match and invalid selectors as not_found", () => {
		expect(invokeSelectorResult(beacon({ count: 0, element: null }), "tap").tier).toBe("not_found");
		const inv = invokeSelectorResult(beacon({ count: 0, element: null, invalid: true }), "tap");
		expect(inv).toMatchObject({ tier: "not_found", invoked: false, invalidSelector: true });
	});
	it("turns busy into an error", () => {
		expect(() => invokeSelectorResult(beacon({ busy: true, count: 0 }), "tap")).toThrow(BUSY_ERROR);
	});
});

describe("invokeAnchorResult", () => {
	const placed = (p: Partial<ShowElementResult>): ShowElementResult => ({
		tier: "anchored",
		highlighted: true,
		element: el,
		...p,
	});
	it("invokes an accepted tier on a confirmed single match", () => {
		for (const tier of ["anchored", "reanchored"] as const) {
			expect(invokeAnchorResult(placed({ tier }), beacon({ domChanged: true }), "tap")).toEqual({
				tier,
				invoked: true,
				action: "tap",
				element: el,
				visible: false,
				domChanged: true,
			});
		}
	});
	it("never acts on a tier show_element would not highlight", () => {
		const stale = placed({ tier: "stale", highlighted: false, score: 0.4 });
		expect(invokeAnchorResult(stale, beacon({}), "tap").invoked).toBe(false);
		const lost = placed({ tier: "not_found", highlighted: false, element: null });
		expect(invokeAnchorResult(lost, null, "tap")).toMatchObject({
			tier: "not_found",
			invoked: false,
		});
	});
	it("does not claim an invoke the frame did not confirm", () => {
		expect(invokeAnchorResult(placed({}), null, "tap").invoked).toBe(false);
		expect(invokeAnchorResult(placed({}), beacon({ count: 2 }), "tap").invoked).toBe(false);
	});
	it("turns busy into an error", () => {
		expect(() => invokeAnchorResult(placed({}), beacon({ busy: true }), "tap")).toThrow(BUSY_ERROR);
	});
});
