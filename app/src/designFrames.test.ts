import { describe, expect, it } from "vitest";
import SRC from "../../crates/skein-daemon/src/design/frames.js?raw";

type Rect = { x: number; y: number; w: number; h: number };
type D = { selector: string; rect: Rect; frame?: string[] };
type Found = { bySelector: D | null; byOdId: D[]; byText: D[]; sameTag: D[] };
type A = { id: string; selector?: string; frame?: string[] };
type Frames = {
	toParentRect: (r: Rect, box: { x: number; y: number }, s: { x: number; y: number }) => Rect;
	prefixDescriptor: (
		d: D,
		sel: string,
		box: { x: number; y: number },
		s: { x: number; y: number },
	) => D;
	partitionAnchors: (a: A[]) => { local: A[]; byFrame: { sel: string; anchors: A[] }[] };
	relayLocated: (
		f: Found | null,
		sel: string,
		box: { x: number; y: number },
		s: { x: number; y: number },
	) => Found;
	emptyFound: () => Found;
};

function load(): Frames {
	const sandbox: { __skeinFrames?: Frames } = {};
	new Function("window", SRC)(sandbox);
	if (!sandbox.__skeinFrames) throw new Error("frames.js did not register");
	return sandbox.__skeinFrames;
}

const F = load();
const d = (selector: string, x = 10, y = 20, frame?: string[]): D => ({
	selector,
	rect: { x, y, w: 5, h: 6 },
	...(frame ? { frame } : {}),
});

describe("toParentRect", () => {
	it("subtracts child scroll and adds the content-box origin", () => {
		expect(
			F.toParentRect({ x: 110, y: 220, w: 30, h: 40 }, { x: 8, y: 9 }, { x: 100, y: 200 }),
		).toEqual({
			x: 18,
			y: 29,
			w: 30,
			h: 40,
		});
	});
	it("tolerates missing scroll", () => {
		expect(F.toParentRect({ x: 1, y: 2, w: 3, h: 4 }, { x: 5, y: 5 }, undefined as never).x).toBe(
			6,
		);
	});
});

describe("prefixDescriptor", () => {
	it("adds the frame and converts the rect without mutating", () => {
		const src = d("#a");
		const out = F.prefixDescriptor(src, "iframe", { x: 100, y: 50 }, { x: 0, y: 0 });
		expect(out.frame).toEqual(["iframe"]);
		expect(out.rect).toEqual({ x: 110, y: 70, w: 5, h: 6 });
		expect(src.frame).toBeUndefined();
	});
	it("composes across two levels", () => {
		const inner = F.prefixDescriptor(d("#a"), "#b", { x: 10, y: 10 }, { x: 0, y: 0 });
		const outer = F.prefixDescriptor(inner, "#a-frame", { x: 100, y: 200 }, { x: 0, y: 5 });
		expect(outer.frame).toEqual(["#a-frame", "#b"]);
		expect(outer.rect).toEqual({ x: 120, y: 225, w: 5, h: 6 });
	});
});

describe("partitionAnchors", () => {
	it("splits local from per-frame groups and drops an emptied frame", () => {
		const { local, byFrame } = F.partitionAnchors([
			{ id: "1", selector: "a" },
			{ id: "2", selector: "b", frame: [] },
			{ id: "3", selector: "c", frame: ["#f"] },
			{ id: "4", selector: "d", frame: ["#f", "#g"] },
			{ id: "5", selector: "e", frame: ["#h"] },
		]);
		expect(local.map((a) => a.id)).toEqual(["1", "2"]);
		expect(byFrame.map((g) => g.sel)).toEqual(["#f", "#h"]);
		expect(byFrame[0]?.anchors).toEqual([
			{ id: "3", selector: "c" },
			{ id: "4", selector: "d", frame: ["#g"] },
		]);
		expect(byFrame[0]?.anchors[0]).not.toHaveProperty("frame");
	});
});

describe("relayLocated", () => {
	it("prefixes all four lists", () => {
		const found: Found = {
			bySelector: d("a"),
			byOdId: [d("b")],
			byText: [d("c"), d("d")],
			sameTag: [d("e")],
		};
		const out = F.relayLocated(found, "#f", { x: 1, y: 1 }, { x: 0, y: 0 });
		for (const x of [out.bySelector, ...out.byOdId, ...out.byText, ...out.sameTag]) {
			expect(x?.frame).toEqual(["#f"]);
			expect(x?.rect.x).toBe(11);
		}
		expect(out.byText).toHaveLength(2);
	});
	it("handles null and missing lists", () => {
		expect(F.relayLocated(null, "#f", { x: 0, y: 0 }, { x: 0, y: 0 })).toEqual(F.emptyFound());
		const out = F.relayLocated({ bySelector: null } as Found, "#f", { x: 0, y: 0 }, { x: 0, y: 0 });
		expect(out).toEqual(F.emptyFound());
	});
});
