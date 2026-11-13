import { beforeEach, describe, expect, it } from "vitest";
import { commitMount, mountKeyFor, resetMounts, restartFor } from "./harnessMounts.ts";

describe("mountKeyFor", () => {
	const tool = ["claude", "--x"];
	const shell = ["/bin/zsh", "-l"];

	it("tool -> shell -> tool: the ends match only when spawnGen matches", () => {
		const first = mountKeyFor("h1", 0, tool);
		const mid = mountKeyFor("h1", 1, shell);
		expect(new Set([first, mid, mountKeyFor("h1", 2, tool)]).size).toBe(3);
		expect(mountKeyFor("h1", 0, tool)).toBe(first);
		expect(mountKeyFor("h1", 2, tool)).not.toBe(first);
	});

	it("a launch change alone (same spawnGen) changes the key", () => {
		expect(mountKeyFor("h1", 1, shell)).not.toBe(mountKeyFor("h1", 1, tool));
	});

	it("treats an absent spawnGen as 0 and differs per harness", () => {
		expect(mountKeyFor("h1", undefined, tool)).toBe(mountKeyFor("h1", 0, tool));
		expect(mountKeyFor("h2", 0, tool)).not.toBe(mountKeyFor("h1", 0, tool));
	});
});

describe("restartFor / commitMount", () => {
	beforeEach(resetMounts);

	it("is not a restart on the first mount", () => {
		expect(restartFor("h1", "k1")).toBe(false);
	});

	it("reading the flag does not record the key", () => {
		restartFor("h1", "k1");
		commitMount("h1", "k1");
		expect(restartFor("h1", "k2")).toBe(true);
		expect(restartFor("h1", "k2")).toBe(true);
	});

	it("is not a restart on a re-mount with the same key", () => {
		commitMount("h1", "k1");
		expect(restartFor("h1", "k1")).toBe(false);
		expect(restartFor("h1", "k1")).toBe(false);
	});

	it("is a restart when the committed key changes, once per change", () => {
		commitMount("h1", "k1");
		expect(restartFor("h1", "k2")).toBe(true);
		commitMount("h1", "k2");
		expect(restartFor("h1", "k2")).toBe(false);
		expect(restartFor("h1", "k1")).toBe(true);
	});

	it("a cancelled mount, then a mount with the same key, still restarts", () => {
		commitMount("h1", "k1");
		// StrictMode run 1: flag computed, cancelled before the ensure call.
		expect(restartFor("h1", "k2")).toBe(true);
		// Run 2 with the same key must still see the restart.
		expect(restartFor("h1", "k2")).toBe(true);
		commitMount("h1", "k2"); // the call is dispatched
		expect(restartFor("h1", "k2")).toBe(false);
	});

	it("tracks harnesses independently", () => {
		commitMount("h1", "k1");
		expect(restartFor("h2", "other")).toBe(false);
		expect(restartFor("h1", "k2")).toBe(true);
		commitMount("h2", "other");
		expect(restartFor("h2", "other")).toBe(false);
	});

	it("resetMounts forgets everything (a page reload starts empty)", () => {
		commitMount("h1", "k1");
		resetMounts();
		expect(restartFor("h1", "k2")).toBe(false);
	});
});
