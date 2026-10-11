import { beforeEach, describe, expect, it } from "vitest";
import {
	effectiveProfile,
	forgetSpawnedProfile,
	profileIsStale,
	recordSpawnedProfile,
	spawnedProfile,
} from "./spawnedEnvProfile.ts";

describe("spawned profile registry", () => {
	beforeEach(() => {
		forgetSpawnedProfile("h1");
		forgetSpawnedProfile("h2");
	});

	it("is undefined before any spawn", () => {
		expect(spawnedProfile("h1")).toBeUndefined();
	});

	it("records names and null per harness", () => {
		recordSpawnedProfile("h1", "work");
		recordSpawnedProfile("h2", null);
		expect(spawnedProfile("h1")).toBe("work");
		expect(spawnedProfile("h2")).toBeNull();
	});

	it("overwrites and forgets", () => {
		recordSpawnedProfile("h1", "a");
		recordSpawnedProfile("h1", "b");
		expect(spawnedProfile("h1")).toBe("b");
		forgetSpawnedProfile("h1");
		expect(spawnedProfile("h1")).toBeUndefined();
	});
});

describe("profileIsStale", () => {
	it("is false with no recorded spawn", () => {
		expect(profileIsStale(undefined, "a")).toBe(false);
	});
	it("is false when equal, including null", () => {
		expect(profileIsStale("a", "a")).toBe(false);
		expect(profileIsStale(null, null)).toBe(false);
	});
	it("is true when they differ", () => {
		expect(profileIsStale("a", "b")).toBe(true);
		expect(profileIsStale(null, "b")).toBe(true);
		expect(profileIsStale("a", null)).toBe(true);
	});
});

describe("effectiveProfile", () => {
	const known = ["a", "b"];
	it("prefers a known override", () => {
		expect(effectiveProfile("b", "a", known)).toBe("b");
	});
	it("falls to active when the override is unknown or absent", () => {
		expect(effectiveProfile("gone", "a", known)).toBe("a");
		expect(effectiveProfile(undefined, "a", known)).toBe("a");
	});
	it("is null when nothing known applies", () => {
		expect(effectiveProfile("gone", "also-gone", known)).toBeNull();
		expect(effectiveProfile(undefined, null, known)).toBeNull();
		expect(effectiveProfile(undefined, undefined, known)).toBeNull();
	});
});

describe("effectiveProfile name trimming", () => {
	it("matches Rust, which trims names", () => {
		expect(effectiveProfile(" a ", null, ["a"])).toBe("a");
		expect(effectiveProfile(undefined, "a", [" a"])).toBe("a");
		expect(effectiveProfile("gone", " ", ["a"])).toBeNull();
	});
	it("returns null while nothing is known (callers gate on the store's loaded flag)", () => {
		expect(effectiveProfile("x", "x", [])).toBeNull();
	});
});
