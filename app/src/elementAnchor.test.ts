import { describe, expect, it } from "vitest";
import {
	type ElementAnchor,
	type ElementDescriptor,
	type LocateResult,
	matchElement,
	normalizeText,
} from "./elementAnchor";

const rect = { x: 0, y: 0, w: 10, h: 10 };

function el(selector: string, over: Partial<ElementDescriptor> = {}): ElementDescriptor {
	return { selector, tag: "button", text: "Buy", attrs: {}, rect, ...over };
}

function anchor(over: Partial<ElementAnchor> = {}): ElementAnchor {
	return { ...el("main > button"), entry: "index.html", ...over };
}

function found(over: Partial<LocateResult> = {}): LocateResult {
	return { bySelector: null, byOdId: [], byText: [], sameTag: [], ...over };
}

type Case = {
	name: string;
	anchor: ElementAnchor;
	found: LocateResult;
	state: "anchored" | "reanchored" | "stale" | "lost";
	at: string | null;
};

const cases: Case[] = [
	{
		name: "same selector, tag and text is anchored",
		anchor: anchor(),
		found: found({ bySelector: el("main > button") }),
		state: "anchored",
		at: "main > button",
	},
	{
		name: "whitespace differences in text still anchor",
		anchor: anchor({ text: "Buy  now" }),
		found: found({ bySelector: el("main > button", { text: " Buy\nnow " }) }),
		state: "anchored",
		at: "main > button",
	},
	{
		name: "unique odId anchors even when the selector moved",
		anchor: anchor({ odId: "od-1" }),
		found: found({ byOdId: [el("section > button", { odId: "od-1" })] }),
		state: "anchored",
		at: "section > button",
	},
	{
		name: "odId match with the wrong tag does not anchor",
		anchor: anchor({ odId: "od-1" }),
		found: found({ byOdId: [el("a", { odId: "od-1", tag: "a" })] }),
		state: "lost",
		at: null,
	},
	{
		name: "anchor odId set and selector has a different odId: not tier 1",
		anchor: anchor({ odId: "od-1" }),
		found: found({
			bySelector: el("main > button", { odId: "od-2" }),
			byText: [el("main > button", { odId: "od-2" })],
		}),
		state: "reanchored",
		at: "main > button",
	},
	{
		name: "bySelector with same odId and text anchors",
		anchor: anchor({ odId: "od-1" }),
		found: found({
			bySelector: el("main > button", { odId: "od-1" }),
			byOdId: [el("main > button", { odId: "od-1" }), el("x > button", { odId: "od-1" })],
		}),
		state: "anchored",
		at: "main > button",
	},
	{
		name: "same selector but different text is not anchored",
		anchor: anchor(),
		found: found({ bySelector: el("main > button", { text: "Sell" }) }),
		state: "lost",
		at: null,
	},
	{
		name: "siblings reordered: selector points at another, found by text",
		anchor: anchor({ text: "Buy" }),
		found: found({
			bySelector: el("main > button", { text: "Sell" }),
			byText: [el("main > button:nth-child(2)", { text: "Buy" })],
			sameTag: [
				el("main > button", { text: "Sell" }),
				el("main > button:nth-child(2)", { text: "Buy" }),
			],
		}),
		state: "reanchored",
		at: "main > button:nth-child(2)",
	},
	{
		name: "odId renamed but text intact: reanchored by text",
		anchor: anchor({ odId: "old" }),
		found: found({
			byText: [el("footer > button", { odId: "new" })],
			sameTag: [el("footer > button", { odId: "new" })],
		}),
		state: "reanchored",
		at: "footer > button",
	},
	{
		name: "several odId hits, one with the same text: reanchored",
		anchor: anchor({ odId: "od-1" }),
		found: found({
			byOdId: [
				el("a > button", { odId: "od-1", text: "Other" }),
				el("b > button", { odId: "od-1", text: "Buy" }),
			],
		}),
		state: "reanchored",
		at: "b > button",
	},
	{
		name: "several odId hits with duplicate text: ambiguous, lost",
		anchor: anchor({ odId: "od-1" }),
		found: found({
			byOdId: [
				el("a > button", { odId: "od-1", text: "Buy" }),
				el("b > button", { odId: "od-1", text: "Buy" }),
			],
			byText: [el("a > button"), el("b > button")],
			sameTag: [el("a > button"), el("b > button")],
		}),
		state: "lost",
		at: null,
	},
	{
		name: "duplicate texts are not reanchored by text",
		anchor: anchor(),
		found: found({
			byText: [el("a > button"), el("b > button")],
			sameTag: [el("a > button"), el("b > button")],
		}),
		state: "lost",
		at: null,
	},
	{
		name: "the same element listed twice still counts as one",
		anchor: anchor(),
		found: found({
			byText: [el("a > button"), el("a > button")],
			sameTag: [el("a > button")],
		}),
		state: "reanchored",
		at: "a > button",
	},
	{
		name: "text gone, unique exact attrs: reanchored",
		anchor: anchor({ attrs: { class: "cta", type: "submit" } }),
		found: found({
			sameTag: [
				el("a > button", { text: "Pay", attrs: { class: "cta", type: "submit" } }),
				el("b > button", { text: "Pay", attrs: { class: "x" } }),
			],
		}),
		state: "reanchored",
		at: "a > button",
	},
	{
		name: "two candidates with the same attrs: not reanchored by attrs",
		anchor: anchor({ attrs: { class: "cta" } }),
		found: found({
			sameTag: [
				el("a > button", { text: "Pay", attrs: { class: "cta" } }),
				el("b > button", { text: "Go", attrs: { class: "cta" } }),
			],
		}),
		state: "lost",
		at: null,
	},
	{
		name: "empty anchor attrs never match by attrs",
		anchor: anchor({ text: "Gone", attrs: {} }),
		found: found({ sameTag: [el("a > button", { text: "Zzz", attrs: {} })] }),
		state: "lost",
		at: null,
	},
	{
		name: "text gone, unique same source file+line: reanchored",
		anchor: anchor({ text: "Gone", source: { file: "a.html", line: 7 } }),
		found: found({
			sameTag: [
				el("a > button", { text: "x", source: { file: "a.html", line: 7 } }),
				el("b > button", { text: "y", source: { file: "a.html", line: 9 } }),
			],
		}),
		state: "reanchored",
		at: "a > button",
	},
	{
		name: "deleted element: nothing left, lost",
		anchor: anchor(),
		found: found(),
		state: "lost",
		at: null,
	},
	{
		name: "empty anchor text does not match empty elements by text",
		anchor: anchor({ text: "" }),
		found: found({
			byText: [el("a > button", { text: "" }), el("b > button", { text: "" })],
			sameTag: [el("a > button", { text: "" })],
		}),
		state: "lost",
		at: null,
	},
	{
		name: "empty anchor text does not reanchor to a lone empty element",
		anchor: anchor({ text: "" }),
		found: found({ byText: [el("a > button", { text: "" })] }),
		state: "lost",
		at: null,
	},
	{
		name: "empty text still anchors on an unchanged selector",
		anchor: anchor({ text: "" }),
		found: found({ bySelector: el("main > button", { text: "" }) }),
		state: "anchored",
		at: "main > button",
	},
	{
		name: "changed copy with shared words and attrs: stale guess",
		anchor: anchor({
			text: "Buy it now today",
			attrs: { class: "cta", id: "b" },
		}),
		found: found({
			sameTag: [
				el("a > button", {
					text: "Buy it now please",
					attrs: { class: "cta", id: "b", extra: "1" },
				}),
				el("b > button", { text: "Cancel", attrs: {} }),
			],
		}),
		state: "stale",
		at: "a > button",
	},
	{
		// text J = 3/5 -> 0.3, attrs J = 2/3 -> 0.2: exactly the threshold
		name: "overlap exactly at the threshold is stale",
		anchor: anchor({ text: "a b c d", attrs: { p: "1", q: "2" } }),
		found: found({
			sameTag: [el("x > button", { text: "a b c e", attrs: { p: "1", q: "2", r: "3" } })],
		}),
		state: "stale",
		at: "x > button",
	},
	{
		// text J = 3/5 -> 0.3, attrs J = 1/2 -> 0.15: below
		name: "overlap just under the threshold is lost",
		anchor: anchor({ text: "a b c d", attrs: { p: "1", q: "2" } }),
		found: found({
			sameTag: [el("x > button", { text: "a b c e", attrs: { p: "1" } })],
		}),
		state: "lost",
		at: null,
	},
	{
		name: "a tie at the top is ambiguous, lost",
		anchor: anchor({ text: "a b c d", attrs: { p: "1" } }),
		found: found({
			sameTag: [
				el("x > button", { text: "a b c e", attrs: { p: "1", s: "1" } }),
				el("y > button", { text: "a b c f", attrs: { p: "1", s: "2" } }),
			],
		}),
		state: "lost",
		at: null,
	},
	{
		name: "runner-up strictly behind: best wins as stale",
		anchor: anchor({ text: "a b c d", attrs: { p: "1" } }),
		found: found({
			sameTag: [
				el("x > button", { text: "a b c e", attrs: { p: "1" } }),
				el("y > button", { text: "a b", attrs: { p: "1" } }),
			],
		}),
		state: "stale",
		at: "x > button",
	},
	{
		name: "candidates of another tag are never scored",
		anchor: anchor({ text: "a b c d" }),
		found: found({
			sameTag: [el("x > a", { tag: "a", text: "a b c d" })],
			bySelector: el("main > button", { tag: "a", text: "a b c d" }),
		}),
		state: "lost",
		at: null,
	},
];

describe("matchElement", () => {
	it.each(cases)("$name", (c) => {
		const p = matchElement(c.anchor, c.found);
		expect(p.state).toBe(c.state);
		expect(p.at?.selector ?? null).toBe(c.at);
		if (c.state === "stale") expect(p.score).toBeGreaterThanOrEqual(0.5 - 1e-9);
		else expect(p.score).toBeUndefined();
	});
});

describe("normalizeText", () => {
	it("collapses whitespace and trims", () => {
		expect(normalizeText("  a \n\t b  ")).toBe("a b");
		expect(normalizeText("   ")).toBe("");
	});
});

describe("known limit", () => {
	// LIMIT, not a goal: with no odId, identical siblings share one
	// selector and text, so when one is deleted the survivor sits at the
	// anchor's selector and reads "anchored". Nothing in the evidence can
	// tell which sibling the comment was about.
	it("duplicate identical siblings, one removed, still reads anchored", () => {
		const p = matchElement(
			anchor({ text: "Buy" }),
			found({ bySelector: el("main > button", { text: "Buy" }) }),
		);
		expect(p.state).toBe("anchored");
	});
});
