import { describe, expect, it } from "vitest";
import {
	EMPTY_NEW_ROOM_MEMORY,
	type FolderDefaults,
	type NewRoomMemory,
	defaultsFor,
	recentFolders,
	rememberFolder,
} from "./prefs.ts";

const defaults = (over: Partial<FolderDefaults> = {}): FolderDefaults => ({
	baseBranch: "main",
	harness: "claude",
	branchMode: "worktree",
	lastUsed: 0,
	...over,
});

const memory = (folders: Record<string, FolderDefaults>, last = ""): NewRoomMemory => ({
	last: last || null,
	folders,
});

describe("rememberFolder", () => {
	it("records the folder as last used and keeps the ones before it", () => {
		const first = rememberFolder(EMPTY_NEW_ROOM_MEMORY, "C:/git/skein", {
			baseBranch: "main",
			harness: "claude",
			branchMode: "worktree",
		});
		const second = rememberFolder(first, "C:/git/timeloop", {
			baseBranch: "dev",
			harness: "opencode",
			branchMode: "current",
		});

		expect(second.last).toBe("C:/git/timeloop");
		expect(Object.keys(second.folders).sort()).toEqual(["C:/git/skein", "C:/git/timeloop"]);
		expect(second.folders["C:/git/skein"]?.baseBranch).toBe("main");
	});

	it("overwrites a folder's defaults rather than accumulating entries", () => {
		const once = rememberFolder(EMPTY_NEW_ROOM_MEMORY, "C:/git/skein", {
			baseBranch: "main",
			harness: "claude",
			branchMode: "worktree",
		});
		const twice = rememberFolder(once, "C:/git/skein", {
			baseBranch: "release",
			harness: "opencode",
			branchMode: "current",
		});

		expect(Object.keys(twice.folders)).toEqual(["C:/git/skein"]);
		expect(twice.folders["C:/git/skein"]?.baseBranch).toBe("release");
		expect(twice.folders["C:/git/skein"]?.harness).toBe("opencode");
	});

	it("stamps lastUsed so ordering does not depend on insertion order", () => {
		const before = Date.now();
		const next = rememberFolder(EMPTY_NEW_ROOM_MEMORY, "C:/git/skein", {
			baseBranch: "",
			harness: "files",
			branchMode: "worktree",
		});
		expect(next.folders["C:/git/skein"]?.lastUsed).toBeGreaterThanOrEqual(before);
	});
});

describe("recentFolders", () => {
	it("is empty for a fresh install", () => {
		expect(recentFolders(EMPTY_NEW_ROOM_MEMORY)).toEqual([]);
	});

	it("orders most recently used first, not alphabetically", () => {
		const m = memory({
			"C:/aaa": defaults({ lastUsed: 10 }),
			"C:/zzz": defaults({ lastUsed: 30 }),
			"C:/mmm": defaults({ lastUsed: 20 }),
		});
		expect(recentFolders(m).map((r) => r.folder)).toEqual(["C:/zzz", "C:/mmm", "C:/aaa"]);
	});

	it("caps the list, dropping the oldest", () => {
		const folders: Record<string, FolderDefaults> = {};
		for (let i = 0; i < 12; i++) folders[`C:/f${i}`] = defaults({ lastUsed: i });
		const got = recentFolders(memory(folders), 8);

		expect(got).toHaveLength(8);
		expect(got[0]?.folder).toBe("C:/f11");
		expect(got.map((r) => r.folder)).not.toContain("C:/f0");
	});

	it("carries each folder's defaults, so a row can show its branch", () => {
		const m = memory({
			"C:/git/skein": defaults({ baseBranch: "main", lastUsed: 2 }),
			"D:/notes": defaults({ baseBranch: "", lastUsed: 1 }),
		});
		const got = recentFolders(m);

		// An empty baseBranch is exactly what marks a non-repo folder —
		// the row needs no separate "is a repo" flag (#231).
		expect(got[0]?.defaults.baseBranch).toBe("main");
		expect(got[1]?.defaults.baseBranch).toBe("");
	});

	it("survives a blob written before the folders map existed", () => {
		// #226 shipped a `{ last, repos }` shape on an unmerged branch, and
		// usePersistedState recovers from unparseable JSON but not from
		// JSON that parsed into the wrong shape.
		const legacy = { last: "C:/git/skein", repos: {} } as unknown as NewRoomMemory;
		expect(recentFolders(legacy)).toEqual([]);
		expect(defaultsFor(legacy, "C:/git/skein")).toBeUndefined();
		expect(rememberFolder(legacy, "C:/git/skein", defaults()).folders).toHaveProperty(
			"C:/git/skein",
		);
	});
});

describe("defaultsFor", () => {
	it("finds a known folder and misses an unknown one", () => {
		const m = memory({ "C:/git/skein": defaults({ baseBranch: "release" }) });
		expect(defaultsFor(m, "C:/git/skein")?.baseBranch).toBe("release");
		expect(defaultsFor(m, "C:/git/other")).toBeUndefined();
	});

	it("treats an empty folder as no folder rather than looking it up", () => {
		// The seed is "" on a fresh install; it must not match a stored key.
		const m = memory({ "": defaults({ baseBranch: "wrong" }) });
		expect(defaultsFor(m, "")).toBeUndefined();
	});
});
