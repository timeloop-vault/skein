import { beforeEach, describe, expect, it } from "vitest";
import {
	releasePersists,
	runsKindProgram,
	shellClaim as sc,
	shouldReplaceClaim,
} from "./shellClaim";

const H = "h1";
const start = (
	current: string | undefined,
	sessionId: string | null,
	source: string | null,
	busy = false,
	mainBusy = false,
) => sc.onSessionStart(H, current, { sessionId, source }, busy, mainBusy);

beforeEach(() => {
	sc.forget(H);
});

describe("not in shell mode", () => {
	it("defers to followedSession", () => {
		expect(start("a", "b", "clear")).toEqual({
			kind: "follow",
			sessionId: "b",
			source: "clear",
			claim: false,
			repoint: true,
		});
		expect(start("a", "b", "startup")).toEqual({ kind: "ignore" });
		expect(start("a", "a", "resume")).toEqual({ kind: "ignore" });
	});
	it("accepts permission", () => {
		expect(sc.acceptsPermission(H, "x")).toBe(true);
		expect(sc.isShell(H)).toBe(false);
	});
});

describe("claiming", () => {
	beforeEach(() => sc.noteSpawn(H, "claude", "/bin/zsh"));

	it.each(["startup", "resume", "clear", "fork"])("first %s claims", (source) => {
		expect(start("old", "a", source)).toEqual({
			kind: "follow",
			sessionId: "a",
			source,
			claim: true,
			repoint: true,
		});
	});
	it.each([
		["compact", "a"],
		[null, "a"],
		["startup", null],
		["startup", ""],
	])("ignores source=%s id=%s while unclaimed", (source, id) => {
		expect(start("old", id, source)).toEqual({ kind: "ignore" });
		expect(sc.acceptsPermission(H, "a")).toBe(false);
	});
	it("same id as stored claims without repoint", () => {
		expect(start("a", "a", "resume")).toMatchObject({ claim: true, repoint: false });
		expect(sc.acceptsPermission(H, "a")).toBe(true);
	});
	it("fresh-process flag is one-shot", () => {
		expect(sc.consumeFreshProcess(H)).toBe(false);
		start(undefined, "a", "startup");
		expect(sc.consumeFreshProcess(H)).toBe(true);
		expect(sc.consumeFreshProcess(H)).toBe(false);
	});
});

describe("nested child", () => {
	beforeEach(() => {
		sc.noteSpawn(H, "claude", "/bin/zsh");
		start("old", "A", "resume");
	});
	it("startup from B ignored, permission gated", () => {
		const d = start("A", "B", "startup", true);
		expect(d).toMatchObject({ kind: "probe-then-replace", claimedBusy: true });
		expect(shouldReplaceClaim(true, true)).toBe(false); // transcript exists, busy
		expect(sc.acceptsPermission(H, "B")).toBe(false);
		expect(sc.acceptsPermission(H, "A")).toBe(true);
		expect(sc.acceptsPermission(H, null)).toBe(true);
		expect(sc.acceptsPermission(H, undefined)).toBe(true);
	});
	it("compact and same-id events are ignored", () => {
		expect(start("A", "A", "compact")).toEqual({ kind: "ignore" });
		expect(start("A", "A", "clear")).toEqual({ kind: "ignore" });
	});
	it.each(["resume", "clear", "fork"])(
		"main mid-turn: different-id %s is ignored, claim kept",
		(source) => {
			expect(start("A", "B", source, true, true)).toEqual({ kind: "ignore" });
			expect(sc.claimedId(H)).toBe("A");
			expect(sc.acceptsPermission(H, "B")).toBe(false);
		},
	);
	it.each(["resume", "clear", "fork"])("idle: different-id %s moves the claim", (source) => {
		expect(start("A", "B", source, false)).toMatchObject({ kind: "follow", claim: true });
		expect(sc.claimedId(H)).toBe("B");
	});
	it.each(["resume", "clear", "fork"])(
		"main at prompt with background work: different-id %s moves the claim",
		(source) => {
			expect(start("A", "B", source, true, false)).toMatchObject({ kind: "follow", claim: true });
			expect(sc.claimedId(H)).toBe("B");
		},
	);
	it("busy: same-id resume stays ignored", () => {
		expect(start("A", "A", "resume", true, true)).toEqual({ kind: "ignore" });
		expect(sc.claimedId(H)).toBe("A");
	});
	it("SessionEnd for B is ignored", () => {
		expect(sc.onSessionEnd(H, { sessionId: "B", reason: "other" })).toBe(false);
		expect(sc.acceptsPermission(H, "A")).toBe(true);
	});
});

describe("moving and releasing", () => {
	beforeEach(() => sc.noteSpawn(H, "claude", "/bin/zsh"));

	it.each(["clear", "resume", "fork"])("%s moves the claim", (source) => {
		start("old", "A", "startup");
		expect(start("A", "B", source)).toEqual({
			kind: "follow",
			sessionId: "B",
			source,
			claim: true,
			repoint: true,
		});
		expect(sc.acceptsPermission(H, "B")).toBe(true);
		expect(sc.acceptsPermission(H, "A")).toBe(false);
		expect(sc.consumeFreshProcess(H)).toBe(true); // from the first claim only
		expect(sc.consumeFreshProcess(H)).toBe(false);
	});
	it("a move does not re-arm fresh", () => {
		start("old", "A", "startup");
		sc.consumeFreshProcess(H);
		start("A", "B", "clear");
		expect(sc.consumeFreshProcess(H)).toBe(false);
	});
	it("SessionEnd releases; next claude claims", () => {
		start("old", "A", "startup");
		expect(sc.onSessionEnd(H, { sessionId: "A", reason: "other" })).toBe(true);
		expect(sc.acceptsPermission(H, "A")).toBe(false);
		expect(start("A", "C", "startup")).toMatchObject({ kind: "follow", claim: true });
		expect(sc.acceptsPermission(H, "C")).toBe(true);
	});
	it.each(["clear", "resume"])("SessionEnd reason %s does not release", (reason) => {
		start("old", "A", "startup");
		expect(sc.onSessionEnd(H, { sessionId: "A", reason })).toBe(false);
		expect(sc.acceptsPermission(H, "A")).toBe(true);
	});
});

describe("phantom startup", () => {
	beforeEach(() => sc.noteSpawn(H, "claude", "/bin/zsh"));

	it("second startup after a startup claim asks to probe", () => {
		start("old", "P", "startup");
		expect(start("P", "R", "startup")).toEqual({
			kind: "probe-then-replace",
			claimedId: "P",
			sessionId: "R",
			claimedBusy: false,
		});
		// nothing changed until the caller replaces
		expect(sc.acceptsPermission(H, "P")).toBe(true);
		sc.replaceClaim(H, "R");
		expect(sc.acceptsPermission(H, "R")).toBe(true);
		expect(sc.acceptsPermission(H, "P")).toBe(false);
	});
	it("a resume-sourced claim also gets probe-then-replace", () => {
		start("old", "A", "resume");
		expect(start("A", "R", "startup", true)).toMatchObject({
			kind: "probe-then-replace",
			claimedId: "A",
			claimedBusy: true,
		});
	});
	it("replaceClaim re-arms the fresh flag", () => {
		start("old", "P", "startup");
		expect(sc.consumeFreshProcess(H)).toBe(true); // the phantom's re-point consumed it
		sc.replaceClaim(H, "R");
		expect(sc.claimedId(H)).toBe("R");
		expect(sc.consumeFreshProcess(H)).toBe(true);
		expect(sc.consumeFreshProcess(H)).toBe(false);
	});
});

describe("releasePersists", () => {
	it.each([
		["clear", true],
		["logout", true],
		["prompt_input_exit", true],
		["bypass_permissions_disabled", true],
		["other", false],
		["", false],
		[null, false],
		[undefined, false],
	])("reason %s -> %s", (reason, expected) => {
		expect(releasePersists(reason)).toBe(expected);
	});
});

describe("shouldReplaceClaim", () => {
	it.each([
		["busy + transcript exists", true, true, false],
		["idle + transcript exists", true, false, true],
		["busy + phantom", false, true, true],
		["idle + phantom", false, false, true],
	])("%s", (_n, exists, busy, replace) => {
		expect(shouldReplaceClaim(exists, busy)).toBe(replace);
	});
});

describe("forget", () => {
	it("clears claim, fresh flag and shell mode", () => {
		sc.noteSpawn(H, "claude", "/bin/zsh");
		start("old", "A", "startup");
		sc.forget(H);
		expect(sc.isShell(H)).toBe(false);
		expect(sc.consumeFreshProcess(H)).toBe(false);
		expect(sc.acceptsPermission(H, "zzz")).toBe(true);
		expect(start("a", "b", "clear")).toMatchObject({ claim: false });
	});
	it("a new shell spawn starts unclaimed", () => {
		sc.noteSpawn(H, "claude", "/bin/zsh");
		start("old", "A", "startup");
		sc.noteSpawn(H, "claude", "/bin/zsh");
		expect(sc.acceptsPermission(H, "A")).toBe(false);
	});
});

describe("runsKindProgram", () => {
	it("matches by stem", () => {
		expect(runsKindProgram("claude", "claude")).toBe(true);
		expect(runsKindProgram("C:\\bin\\Claude.EXE", "claude")).toBe(true);
		expect(runsKindProgram("/usr/local/bin/claude", "claude")).toBe(true);
	});
	it("rejects shells and kinds without a program", () => {
		expect(runsKindProgram("/bin/zsh", "claude")).toBe(false);
		expect(runsKindProgram("claude-code", "claude")).toBe(false);
		expect(runsKindProgram(undefined, "claude")).toBe(false);
		expect(runsKindProgram("claude", null)).toBe(false);
	});
});

describe("noteSpawn", () => {
	it("enters shell mode for a non-claude program and leaves for claude", () => {
		sc.noteSpawn(H, "claude", "/bin/zsh");
		expect(sc.isShell(H)).toBe(true);
		sc.noteSpawn(H, "claude", "claude");
		expect(sc.isShell(H)).toBe(false);
	});
	it("ignores other kinds", () => {
		sc.noteSpawn(H, "shell", "/bin/zsh");
		expect(sc.isShell(H)).toBe(false);
	});
});
