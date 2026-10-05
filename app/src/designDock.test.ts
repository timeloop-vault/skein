import { describe, expect, it } from "vitest";
import {
	dockedDesignHarnesses,
	effectiveRightPaneTab,
	shownDesignHarness,
	withDocked,
} from "./designDock.ts";
import type { Harness } from "./types.ts";

const h = (id: string, kind: Harness["kind"]) => ({ id, kind, name: id }) as Harness;
const hs = [h("a", "design"), h("b", "claude"), h("c", "design"), h("d", "design")];

describe("dockedDesignHarnesses", () => {
	it.each([
		["nothing docked", {}, []],
		["only design kinds count", { a: true, b: true }, ["a"]],
		["room order, not dock order", { d: true, a: true }, ["a", "d"]],
		["unknown ids ignored", { zzz: true }, []],
	] as const)("%s", (_n, docked, ids) => {
		expect(dockedDesignHarnesses(hs, docked).map((x) => x.id)).toEqual(ids);
	});
});

describe("shownDesignHarness", () => {
	const list = [hs[0] as Harness, hs[2] as Harness];
	it.each([
		["pick still docked", "c", "c"],
		["stale pick falls to first", "d", "a"],
		["no pick", undefined, "a"],
	] as const)("%s", (_n, pick, id) => {
		expect(shownDesignHarness(list, pick)?.id).toBe(id);
	});
	it("undefined when none docked", () => {
		expect(shownDesignHarness([], "a")).toBeUndefined();
	});
});

describe("effectiveRightPaneTab", () => {
	it.each([
		["design", true, "design"],
		["design", false, "context"],
		["review", false, "review"],
		["context", true, "context"],
	] as const)("%s docked=%s -> %s", (tab, has, want) => {
		expect(effectiveRightPaneTab(tab, has)).toBe(want);
	});
});

describe("withDocked", () => {
	it("docks and undocks immutably", () => {
		const a = withDocked({}, "a", true);
		expect(a).toEqual({ a: true });
		expect(withDocked(a, "a", false)).toEqual({});
	});
	it("returns the same object when unchanged", () => {
		const a = { a: true } as const;
		expect(withDocked(a, "a", true)).toBe(a);
		const e = {};
		expect(withDocked(e, "a", false)).toBe(e);
	});
});
