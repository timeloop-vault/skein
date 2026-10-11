import { describe, expect, it } from "vitest";
import { envProfileMenuItems, overrideToastMessage, staleLine } from "./envProfileMenu.ts";

describe("envProfileMenuItems", () => {
	it("checks follow-global when there is no override", () => {
		const items = envProfileMenuItems(undefined, "work", ["work", "play"]);
		expect(items.map((i) => i.label)).toEqual(["Follow global (work)", "work", "play"]);
		expect(items.filter((i) => i.checked).map((i) => i.id)).toEqual(["follow"]);
		expect(items[0]?.value).toBeUndefined();
	});
	it("says none when no global profile is active", () => {
		expect(envProfileMenuItems(undefined, null, ["a"])[0]?.label).toBe("Follow global (none)");
	});
	it("checks the chosen profile", () => {
		const items = envProfileMenuItems("play", "work", ["work", "play"]);
		expect(items.filter((i) => i.checked).map((i) => i.value)).toEqual(["play"]);
	});
	it("lists a missing override as following global", () => {
		const items = envProfileMenuItems("gone", "work", ["work"]);
		const m = items.find((i) => i.missing);
		expect(m?.label).toBe("gone (missing — following global)");
		expect(m?.checked).toBe(true);
		expect(items[0]?.checked).toBe(false);
	});
});

describe("staleLine", () => {
	it("is null when matching or unspawned", () => {
		expect(staleLine("a", "a")).toBeNull();
		expect(staleLine(undefined, "a")).toBeNull();
	});
	it("words both directions", () => {
		expect(staleLine("a", null)).toBe("Started with a — restart to apply no profile");
		expect(staleLine(null, "b")).toBe("Started with no profile — restart to apply b");
	});
});

describe("overrideToastMessage", () => {
	it("is null when the effective profile matches the spawned one", () => {
		expect(overrideToastMessage("h", "a", "a", null, ["a"])).toBeNull();
		expect(overrideToastMessage("h", undefined, "a", null, ["a"])).toBeNull();
	});
	it("names the new profile or none", () => {
		expect(overrideToastMessage("Claude", null, "a", null, ["a"])).toBe(
			'Claude will use env profile "a" after a restart.',
		);
		expect(overrideToastMessage("Claude", "a", undefined, null, ["a"])).toBe(
			"Claude will use no profile after a restart.",
		);
	});
	it("treats a missing override as the global", () => {
		expect(overrideToastMessage("h", "w", "gone", "w", ["w"])).toBeNull();
	});
});

describe("envProfileMenuItems trimming", () => {
	it("does not call a padded override missing", () => {
		const items = envProfileMenuItems(" work ", null, ["work"]);
		expect(items.some((i) => i.missing)).toBe(false);
		expect(items.find((i) => i.id === "profile:work")?.checked).toBe(true);
	});
});
