import { describe, expect, it } from "vitest";
import {
	type ElementThread,
	buildLocateAnchors,
	buildPins,
	placeThreads,
	placementSignature,
	seenWrites,
} from "./designComments.ts";
import type { ElementDescriptor, LocateResult } from "./elementAnchor.ts";

const rect = { x: 10, y: 20, w: 100, h: 30 };
const desc = (over: Partial<ElementDescriptor> = {}): ElementDescriptor => ({
	selector: "main > h1",
	tag: "h1",
	text: "Hello",
	attrs: {},
	rect,
	...over,
});
const empty: LocateResult = { bySelector: null, byOdId: [], byText: [], sameTag: [] };

const thread = (id: string, over: Partial<ElementThread> = {}): ElementThread =>
	({
		id,
		comments: [],
		element: { anchor: { ...desc(), entry: "a.html" }, state: "unknown" },
		...over,
	}) as unknown as ElementThread;

describe("locate request", () => {
	it("covers open element threads only", () => {
		const threads = [
			thread("a"),
			thread("b", { resolvedMs: 1 }),
			{ id: "c", comments: [] } as unknown as ElementThread,
		];
		expect(buildLocateAnchors(threads).map((a) => a.id)).toEqual(["a"]);
	});

	it("signature ignores lastSeen churn", () => {
		const a = thread("a");
		const b = thread("a");
		if (b.element) b.element.lastSeen = { state: "anchored", seenMs: 1 };
		expect(placementSignature([a])).toBe(placementSignature([b]));
		expect(placementSignature([a])).not.toBe(placementSignature([a, thread("z")]));
	});
});

describe("pins", () => {
	it("numbers by list order and puts lost pins at the last evidence", () => {
		const t1 = thread("a", { resolvedMs: 5 });
		const t2 = thread("b");
		const t3 = thread("c");
		if (t3.element)
			t3.element.lastSeen = { state: "anchored", seenMs: 1, rect: { x: 1, y: 2, w: 3, h: 4 } };
		const placements = placeThreads(
			[t1, t2, t3],
			[
				{ id: "b", found: { ...empty, bySelector: desc() } },
				{ id: "c", found: empty },
			],
		);
		expect(buildPins([t1, t2, t3], placements)).toEqual([
			{ n: 2, state: "anchored", rect },
			{ n: 3, state: "lost", rect: { x: 1, y: 2, w: 3, h: 4 } },
		]);
	});
});

describe("seen write-back", () => {
	const found = [{ id: "a", found: { ...empty, bySelector: desc() } }];

	it("writes when unknown, then not when the store agrees", () => {
		const t = thread("a");
		const p = placeThreads([t], found);
		const w = seenWrites([t], p, ["a.html"], new Set());
		expect(w).toHaveLength(1);
		expect(w[0]?.seen).toMatchObject({
			state: "anchored",
			selector: "main > h1",
			files: ["a.html"],
		});

		if (t.element) {
			t.element.state = "anchored";
			t.element.lastSeen = {
				state: "anchored",
				seenMs: 1,
				selector: "main > h1",
				rect: { ...rect, x: 11 },
			};
		}
		expect(seenWrites([t], p, [], new Set())).toEqual([]);
	});

	it("writes on a real change and skips threads already written this load", () => {
		const t = thread("a");
		if (t.element) {
			t.element.state = "anchored";
			t.element.lastSeen = {
				state: "anchored",
				seenMs: 1,
				selector: "main > h1",
				rect: { ...rect, y: 90 },
			};
		}
		const p = placeThreads([t], found);
		expect(seenWrites([t], p, [], new Set())).toHaveLength(1);
		expect(seenWrites([t], p, [], new Set(["a"]))).toEqual([]);
	});
});
