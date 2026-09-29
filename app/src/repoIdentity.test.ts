import { describe, expect, it } from "vitest";
import { compareIdentity, identityFromDto, isStorable, nextRepoRoot } from "./repoIdentity.ts";

const id = (...c: string[]) => ({ rootCommits: c });

describe("compareIdentity", () => {
	const cases: [string, Parameters<typeof compareIdentity>, string][] = [
		["no stored", [undefined, { exists: true, identity: id("a") }], "unknown"],
		["empty stored", [id(), { exists: true, identity: id("a") }], "unknown"],
		["folder missing", [id("a"), { exists: false, identity: null }], "unknown"],
		["not a repo", [id("a"), { exists: true, identity: null }], "mismatch"],
		["unborn", [id("a"), { exists: true, identity: id() }], "mismatch"],
		["shallow", [id("a"), { exists: true, identity: id(), shallow: true }], "unknown"],
		["disjoint", [id("a"), { exists: true, identity: id("b", "c") }], "mismatch"],
		["overlap", [id("a", "b"), { exists: true, identity: id("b") }], "same"],
		["equal", [id("a"), { exists: true, identity: id("a") }], "same"],
	];
	for (const [name, args, want] of cases) {
		it(name, () => expect(compareIdentity(...args)).toBe(want));
	}
});

describe("identityFromDto", () => {
	it("drops null origin", () => {
		expect(identityFromDto({ rootCommits: ["a"], originUrl: null })).toEqual({
			rootCommits: ["a"],
		});
	});
	it("keeps origin", () => {
		expect(identityFromDto({ rootCommits: ["a"], originUrl: "u" })).toEqual({
			rootCommits: ["a"],
			originUrl: "u",
		});
	});
});

describe("isStorable", () => {
	it("needs roots", () => {
		expect(isStorable(null)).toBe(false);
		expect(isStorable(id())).toBe(false);
		expect(isStorable(id("a"))).toBe(true);
	});
});

describe("nextRepoRoot", () => {
	it("fills when none", () => expect(nextRepoRoot(undefined, "/r", "unknown")).toBe("/r"));
	it("keeps when nothing derived", () => expect(nextRepoRoot("/o", undefined, "same")).toBe("/o"));
	it("re-derives only when same", () => {
		expect(nextRepoRoot("/o", "/n", "same")).toBe("/n");
		expect(nextRepoRoot("/o", "/n", "unknown")).toBe("/o");
		expect(nextRepoRoot("/o", "/n", "mismatch")).toBe("/o");
	});
});
