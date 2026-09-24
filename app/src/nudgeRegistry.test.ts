import { describe, expect, it } from "vitest";
import {
	NUDGES,
	type NudgeDef,
	type NudgeOverrides,
	actionNudges,
	isOverridden,
	nudgeBody,
	nudgesInScope,
	withOverride,
	withoutOverride,
} from "./nudgeRegistry.ts";

const def = (over: Partial<NudgeDef> = {}): NudgeDef => ({
	id: "test-nudge",
	label: "Nudge: test",
	description: "a test nudge",
	defaultBody: "default body",
	scope: "review",
	...over,
});

describe("nudgeBody", () => {
	it("falls back to the default body with no overrides", () => {
		expect(nudgeBody(def())).toBe("default body");
		expect(nudgeBody(def(), {})).toBe("default body");
	});

	it("returns the override verbatim when one is present", () => {
		const overrides: NudgeOverrides = { "test-nudge": "  custom body  " };
		expect(nudgeBody(def(), overrides)).toBe("  custom body  ");
	});

	it("falls back to the default when the override is blank", () => {
		expect(nudgeBody(def(), { "test-nudge": "" })).toBe("default body");
		expect(nudgeBody(def(), { "test-nudge": "   " })).toBe("default body");
	});

	it("ignores an override keyed to a different id", () => {
		expect(nudgeBody(def(), { "other-nudge": "custom body" })).toBe("default body");
	});
});

describe("isOverridden", () => {
	it("is false with no override and true with one", () => {
		expect(isOverridden(def())).toBe(false);
		expect(isOverridden(def(), { "test-nudge": "custom" })).toBe(true);
	});

	it("is false for a blank override", () => {
		expect(isOverridden(def(), { "test-nudge": "  " })).toBe(false);
	});
});

describe("withOverride / withoutOverride", () => {
	it("stores a new override", () => {
		const next = withOverride({}, "review-land", "custom land body");
		expect(next).toEqual({ "review-land": "custom land body" });
	});

	it("removes the key when the body is edited back to the registry default", () => {
		const withIt = withOverride({}, "review-land", "custom land body");
		const backToDefault = withOverride(
			withIt,
			"review-land",
			"The review is signed off. Land this branch per this repo's conventions.",
		);
		expect(backToDefault).toEqual({});
	});

	it("removes the key for a blank body", () => {
		const withIt = withOverride({}, "review-land", "custom land body");
		expect(withOverride(withIt, "review-land", "")).toEqual({});
		expect(withOverride(withIt, "review-land", "   ")).toEqual({});
	});

	it("stores an override for an id NUDGES doesn't know, since only blank is checked", () => {
		expect(withOverride({}, "unknown-id", "some body")).toEqual({ "unknown-id": "some body" });
	});

	it("withoutOverride resets a known id and is a no-op for one that isn't set", () => {
		const withIt = withOverride({}, "review-land", "custom land body");
		expect(withoutOverride(withIt, "review-land")).toEqual({});
		expect(withoutOverride({}, "review-land")).toEqual({});
	});

	it("leaves other overrides untouched", () => {
		const both = withOverride(
			withOverride({}, "review-land", "land override"),
			"review-lapsed",
			"lapsed override",
		);
		expect(withoutOverride(both, "review-land")).toEqual({ "review-lapsed": "lapsed override" });
	});
});

describe("unknown ids in a stored override map", () => {
	it("are simply ignored — a removed nudge id never breaks anything reading through NUDGES", () => {
		const overrides: NudgeOverrides = { "removed-nudge-from-an-old-version": "stale text" };
		// Nothing in NUDGES matches that id, so every def resolves to its
		// own default — the stale entry is inert, not an error.
		for (const d of NUDGES) {
			expect(nudgeBody(d, overrides)).toBe(d.defaultBody);
		}
	});
});

describe("nudgesInScope / actionNudges", () => {
	it("NUDGES today is entirely the review scope", () => {
		expect(nudgesInScope("review")).toHaveLength(NUDGES.length);
		expect(nudgesInScope("review").every((d) => d.scope === "review")).toBe(true);
	});

	it("the actions scope is empty today", () => {
		expect(nudgesInScope("actions")).toEqual([]);
		expect(actionNudges()).toEqual([]);
	});
});
