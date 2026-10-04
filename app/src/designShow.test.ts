import { describe, expect, it, vi } from "vitest";
import { confirmHighlight, PendingRequests, selectorResult } from "./designShow.ts";
import type { ElementDescriptor } from "./elementAnchor.ts";

const el: ElementDescriptor = {
	selector: "#a",
	tag: "div",
	text: "hi",
	attrs: {},
	rect: { x: 0, y: 0, w: 1, h: 1 },
};

describe("selectorResult", () => {
	it("reports an invalid selector", () => {
		expect(selectorResult(0, null, true)).toEqual({
			tier: "not_found",
			highlighted: false,
			element: null,
			invalidSelector: true,
		});
	});

	it("unique match is the selector tier", () => {
		expect(selectorResult(1, el)).toEqual({
			tier: "selector",
			highlighted: true,
			element: el,
			count: 1,
		});
	});
	it("several matches are ambiguous and not highlighted", () => {
		expect(selectorResult(3, null)).toEqual({
			tier: "ambiguous",
			highlighted: false,
			element: null,
			count: 3,
		});
	});
	it("none, or a unique count without a descriptor, is not_found", () => {
		expect(selectorResult(0, null).tier).toBe("not_found");
		expect(selectorResult(1, null).tier).toBe("not_found");
	});
});

describe("confirmHighlight", () => {
	const placed = { tier: "exact", highlighted: true, element: el, count: 1 } as never;
	it("keeps the claim when the frame confirms one match", () => {
		expect(confirmHighlight(placed, { count: 1 })).toBe(placed);
	});
	it("drops the claim when the round trip failed or count is not 1", () => {
		for (const shown of [null, { count: 0 }, { count: 2 }]) {
			const r = confirmHighlight(placed, shown);
			expect(r.highlighted).toBe(false);
			expect(r.element).toBe(el);
			expect(r.tier).toBe("exact");
		}
	});
});

describe("PendingRequests", () => {
	it("resolves on settle", async () => {
		const p = new PendingRequests<number>("x-");
		let id = "";
		const r = p.begin((i) => {
			id = i;
		});
		expect(p.owns(id)).toBe(true);
		expect(p.settle(id, 7)).toBe(true);
		expect(await r).toBe(7);
		expect(p.settle(id, 8)).toBe(false);
	});
	it("rejects with not_ready on timeout", async () => {
		vi.useFakeTimers();
		const p = new PendingRequests<number>("x-", 100);
		const r = p.begin(() => {});
		const assertion = expect(r).rejects.toThrow(/^not_ready: /);
		await vi.advanceTimersByTimeAsync(101);
		await assertion;
		vi.useRealTimers();
	});
	it("rejects when send throws and on rejectAll", async () => {
		const p = new PendingRequests<number>("x-");
		await expect(
			p.begin(() => {
				throw new Error("boom");
			}),
		).rejects.toThrow(/^not_ready: /);
		const r = p.begin(() => {});
		p.rejectAll("unmounted");
		await expect(r).rejects.toThrow(/^not_ready: unmounted/);
	});
});
