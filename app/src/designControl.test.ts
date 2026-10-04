import { describe, expect, it } from "vitest";
import {
	type DesignPaneApi,
	paneSummaries,
	parseOpenEntryArgs,
	parsePanesArgs,
	parseShowElementArgs,
	parseStateArgs,
	registerDesignPane,
	showResultFromPlacement,
} from "./designControl.ts";
import type { ElementAnchor, ElementDescriptor } from "./elementAnchor.ts";

const el: ElementDescriptor = {
	selector: "div > p",
	tag: "p",
	text: "hi",
	attrs: {},
	rect: { x: 1, y: 2, w: 3, h: 4 },
};
const anchor: ElementAnchor = { ...el, entry: "index.html" };

const pane = (roomId: string, ready: boolean): DesignPaneApi => ({
	roomId,
	ready: () => ready,
	getState: () => ({
		entry: "index.html",
		device: null,
		ready,
		loadFailed: false,
		errors: [],
		selected: null,
		scroll: null,
	}),
	showElement: async () => ({ tier: "selector", highlighted: true, element: el }),
});

describe("registry", () => {
	it("lists panes per room and disposes", () => {
		const d1 = registerDesignPane("h1", pane("r1", true));
		const d2 = registerDesignPane("h2", pane("r1", false));
		const d3 = registerDesignPane("h3", pane("r2", true));
		expect(paneSummaries("r1")).toEqual([
			{ harnessId: "h1", mounted: true, ready: true },
			{ harnessId: "h2", mounted: true, ready: false },
		]);
		d1();
		expect(paneSummaries("r1").map((p) => p.harnessId)).toEqual(["h2"]);
		d2();
		d3();
		expect(paneSummaries("r2")).toEqual([]);
	});

	it("a stale disposer does not remove a newer registration", () => {
		const first = registerDesignPane("h9", pane("r9", true));
		const second = registerDesignPane("h9", pane("r9", true));
		first();
		expect(paneSummaries("r9")).toHaveLength(1);
		second();
		expect(paneSummaries("r9")).toHaveLength(0);
	});
});

describe("parsers", () => {
	it("panes/state require ids", () => {
		expect(parsePanesArgs({ roomId: "r" }).ok).toBe(true);
		expect(parsePanesArgs({}).ok).toBe(false);
		expect(parsePanesArgs(null).ok).toBe(false);
		expect(parseStateArgs({ roomId: "r" }).ok).toBe(false);
		expect(parseStateArgs({ roomId: "r", harnessId: "h" }).ok).toBe(true);
	});
	it("open_entry needs an entry", () => {
		expect(parseOpenEntryArgs({ roomId: "r", harnessId: "h" }).ok).toBe(false);
		expect(parseOpenEntryArgs({ roomId: "r", harnessId: "h", entry: "a.html" })).toEqual({
			ok: true,
			value: { roomId: "r", harnessId: "h", entry: "a.html" },
		});
	});
	it("show_element accepts null for the absent one and needs one of the two", () => {
		const base = { roomId: "r", harnessId: "h" };
		expect(parseShowElementArgs({ ...base, selector: "p", anchor: null })).toEqual({
			ok: true,
			value: { ...base, selector: "p" },
		});
		const a = parseShowElementArgs({ ...base, selector: null, anchor });
		expect(a.ok && a.value.anchor).toMatchObject({
			selector: anchor.selector,
			tag: anchor.tag,
			text: anchor.text,
		});
		expect(parseShowElementArgs({ ...base, selector: null, anchor: null }).ok).toBe(false);
		expect(parseShowElementArgs({ ...base, selector: 3 }).ok).toBe(false);
		const min = parseShowElementArgs({ ...base, anchor: { selector: "x" } });
		expect(min.ok && min.value.anchor?.selector).toBe("x");
		const tagOnly = parseShowElementArgs({ ...base, anchor: { tag: "p", text: "hi" } });
		expect(tagOnly.ok && tagOnly.value.anchor?.attrs).toEqual({});
		const none = parseShowElementArgs({ ...base, anchor: { text: "hi" } });
		expect(!none.ok && none.error.startsWith("bad_arguments: ")).toBe(true);
		expect(parseShowElementArgs({ ...base, anchor: { tag: "p", attrs: { a: 1 } } }).ok).toBe(false);
	});
});

describe("showResultFromPlacement", () => {
	it("maps each state", () => {
		expect(showResultFromPlacement({ state: "anchored", at: el })).toEqual({
			tier: "anchored",
			highlighted: true,
			element: el,
		});
		expect(showResultFromPlacement({ state: "reanchored", at: el })).toEqual({
			tier: "reanchored",
			highlighted: true,
			element: el,
		});
		expect(showResultFromPlacement({ state: "stale", at: el, score: 0.6 })).toEqual({
			tier: "stale",
			highlighted: false,
			element: el,
			score: 0.6,
		});
		expect(showResultFromPlacement({ state: "lost", at: null })).toEqual({
			tier: "not_found",
			highlighted: false,
			element: null,
		});
	});
});
