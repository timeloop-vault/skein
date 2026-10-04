import { afterEach, describe, expect, it } from "vitest";
import { adoptionDecision, startupAdoption, startupCandidate } from "./startupAdoption";

describe("startupCandidate", () => {
	const cases: Array<[string, string | undefined, object, string | null]> = [
		["different startup id", "A", { source: "startup", sessionId: "B" }, "B"],
		["same id", "A", { source: "startup", sessionId: "A" }, null],
		["no bound session", undefined, { source: "startup", sessionId: "B" }, null],
		["not startup", "A", { source: "clear", sessionId: "B" }, null],
		["missing source", "A", { sessionId: "B" }, null],
		["empty id", "A", { source: "startup", sessionId: "" }, null],
		["null id", "A", { source: "startup", sessionId: null }, null],
		["missing id", "A", { source: "startup" }, null],
	];
	it.each(cases)("%s", (_n, current, payload, want) => {
		expect(startupCandidate(current, payload)).toBe(want);
	});
});

describe("adoptionDecision", () => {
	const cases: Array<[boolean, boolean, string]> = [
		[true, true, "drop"],
		[true, false, "drop"],
		[false, true, "adopt"],
		[false, false, "wait"],
	];
	it.each(cases)("bound=%s candidate=%s -> %s", (b, c, want) => {
		expect(adoptionDecision(b, c)).toBe(want);
	});
});

describe("startupAdoption registry", () => {
	afterEach(() => {
		for (const id of startupAdoption.harnessIds()) startupAdoption.forget(id);
	});

	it("is empty by default", () => {
		expect(startupAdoption.pending("h")).toBeNull();
		expect(startupAdoption.harnessIds()).toEqual([]);
	});

	it("notes and dedupes candidates", () => {
		startupAdoption.note("h", "r", "A", "B");
		startupAdoption.note("h", "r", "A", "B");
		startupAdoption.note("h", "r", "A", "C");
		expect(startupAdoption.pending("h")).toEqual({
			roomId: "r",
			bound: "A",
			candidates: ["B", "C"],
		});
		expect(startupAdoption.harnessIds()).toEqual(["h"]);
	});

	it("replaces the entry when bound differs", () => {
		startupAdoption.note("h", "r", "A", "B");
		startupAdoption.note("h", "r", "X", "C");
		expect(startupAdoption.pending("h")).toEqual({ roomId: "r", bound: "X", candidates: ["C"] });
	});

	it("drop removes one candidate and the entry when empty", () => {
		startupAdoption.note("h", "r", "A", "B");
		startupAdoption.note("h", "r", "A", "C");
		startupAdoption.drop("h", "B");
		expect(startupAdoption.pending("h")?.candidates).toEqual(["C"]);
		startupAdoption.drop("h", "C");
		expect(startupAdoption.pending("h")).toBeNull();
		startupAdoption.drop("h", "C");
	});

	it("forget removes the entry", () => {
		startupAdoption.note("h", "r", "A", "B");
		startupAdoption.forget("h");
		expect(startupAdoption.pending("h")).toBeNull();
	});
});
