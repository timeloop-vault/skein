import { describe, expect, it } from "vitest";
import SRC from "../src-tauri/src/design/hit.js?raw";

type Cand = {
	rect: { left: number; top: number; width: number; height: number };
	depth: number;
	order: number;
};
type Hit = {
	pickDeepest: (p: { x: number; y: number }, c: Cand[]) => number;
	isUnstableId: (id: string) => boolean;
	refine: <T>(
		hit: T,
		w: T | null,
		o: { isHost: (x: T) => boolean; contains: (a: T, b: T) => boolean },
	) => T;
	labelOf: (i: { tag?: string; role?: string; ariaLabel?: string; text?: string }) => string;
};

function load(): Hit {
	const sandbox: { __skeinHit?: Hit } = {};
	new Function("window", SRC)(sandbox);
	if (!sandbox.__skeinHit) throw new Error("hit.js did not register");
	return sandbox.__skeinHit;
}

const c = (l: number, t: number, w: number, h: number, depth: number, order: number): Cand => ({
	rect: { left: l, top: t, width: w, height: h },
	depth,
	order,
});

describe("pickDeepest", () => {
	const { pickDeepest } = load();
	it("picks the innermost nested rect", () => {
		const cs = [c(0, 0, 100, 100, 0, 0), c(10, 10, 50, 50, 1, 1), c(20, 20, 10, 10, 2, 2)];
		expect(pickDeepest({ x: 25, y: 25 }, cs)).toBe(2);
		expect(pickDeepest({ x: 50, y: 50 }, cs)).toBe(1);
		expect(pickDeepest({ x: 90, y: 90 }, cs)).toBe(0);
	});
	it("prefers smaller area, then later order, among equal depth", () => {
		expect(pickDeepest({ x: 5, y: 5 }, [c(0, 0, 20, 20, 1, 0), c(0, 0, 10, 10, 1, 1)])).toBe(1);
		expect(pickDeepest({ x: 5, y: 5 }, [c(0, 0, 10, 10, 1, 0), c(0, 0, 20, 20, 1, 1)])).toBe(0);
		expect(pickDeepest({ x: 5, y: 5 }, [c(0, 0, 10, 10, 1, 0), c(0, 0, 10, 10, 1, 1)])).toBe(1);
	});
	it("returns -1 when nothing contains the point", () => {
		expect(pickDeepest({ x: 500, y: 5 }, [c(0, 0, 10, 10, 0, 0)])).toBe(-1);
		expect(pickDeepest({ x: 1, y: 1 }, [])).toBe(-1);
	});
	it("excludes zero-area rects", () => {
		expect(pickDeepest({ x: 5, y: 5 }, [c(0, 0, 10, 10, 0, 0), c(5, 5, 0, 10, 3, 1)])).toBe(0);
		expect(pickDeepest({ x: 5, y: 5 }, [c(5, 5, 0, 0, 3, 0)])).toBe(-1);
	});
	it("includes edges", () => {
		const cs = [c(10, 10, 20, 20, 0, 0)];
		expect(pickDeepest({ x: 10, y: 10 }, cs)).toBe(0);
		expect(pickDeepest({ x: 30, y: 30 }, cs)).toBe(0);
		expect(pickDeepest({ x: 30.1, y: 30 }, cs)).toBe(-1);
	});
});

describe("isUnstableId", () => {
	const { isUnstableId } = load();
	it("flags only Flutter semantic-node counters", () => {
		expect(isUnstableId("flt-semantic-node-12")).toBe(true);
		expect(isUnstableId("flt-semantic-node-")).toBe(false);
		expect(isUnstableId("flt-semantic-node-1a")).toBe(false);
		expect(isUnstableId("hero")).toBe(false);
		expect(isUnstableId("")).toBe(false);
	});
});

describe("labelOf", () => {
	const { labelOf } = load();
	it("joins tag, role and quoted label", () => {
		expect(labelOf({ tag: "flt-semantics", role: "button", ariaLabel: "Save" })).toBe(
			'flt-semantics · button · "Save"',
		);
	});
	it("falls back to text, and omits missing parts", () => {
		expect(labelOf({ tag: "div", text: "hello" })).toBe('div · "hello"');
		expect(labelOf({ tag: "div" })).toBe("div");
		expect(labelOf({ tag: "div", role: "img" })).toBe("div · img");
	});
	it("clips long labels", () => {
		const out = labelOf({ tag: "p", ariaLabel: "x".repeat(100) });
		expect(out).toBe(`p · "${"x".repeat(39)}…"`);
	});
});

describe("refine", () => {
	const { refine } = load();
	const nest: Record<string, string[]> = { a: ["b"], b: [] };
	const contains = (x: string, y: string) => (nest[x] ?? []).includes(y);
	const opts = { isHost: (x: string) => x === "host", contains };
	it("replaces a host target with the rect winner", () => {
		expect(refine("host", "b", opts)).toBe("b");
	});
	it("keeps a host target when there is no winner", () => {
		expect(refine("host", null, opts)).toBe("host");
	});
	it("refines downward to a winner inside a fine hit", () => {
		expect(refine("a", "b", opts)).toBe("b");
	});
	it("keeps the hit when the winner is its ancestor", () => {
		expect(refine("b", "a", opts)).toBe("b");
	});
	it("keeps the hit for an unrelated winner or none", () => {
		expect(refine("b", "z", opts)).toBe("b");
		expect(refine("b", null, opts)).toBe("b");
		expect(refine("a", "a", opts)).toBe("a");
	});
});
