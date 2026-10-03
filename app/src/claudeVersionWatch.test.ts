import { describe, expect, it } from "vitest";
import {
	AUTO_RESTART_SETTLE_MS,
	type AutoRestartCandidate,
	autoRestartTargets,
	PROBE_INTERVAL_MS,
	shouldProbe,
} from "./claudeVersion.ts";
import { versionNoticeModeFor, withVersionNoticeMode } from "./prefs.ts";

describe("shouldProbe", () => {
	it.each([
		[1_000, null, false, true],
		[1_000, null, true, false],
		[PROBE_INTERVAL_MS, 0, false, true],
		[PROBE_INTERVAL_MS - 1, 0, false, false],
		[PROBE_INTERVAL_MS * 2, 0, true, false],
	])("now=%s last=%s inFlight=%s -> %s", (now, last, inFlight, want) => {
		expect(shouldProbe(now, last, inFlight)).toBe(want);
	});
});

const NOW = 1_000_000;
const cand = (over: Partial<AutoRestartCandidate> = {}): AutoRestartCandidate => ({
	roomId: "r1",
	harnessId: "h1",
	notice: { running: "2.1.1", installed: "2.1.2" },
	phase: "waiting",
	autoRestartedFor: null,
	waitingSinceMs: NOW - AUTO_RESTART_SETTLE_MS,
	triedThisEpisode: false,
	...over,
});

describe("autoRestartTargets", () => {
	it("restarts a waiting, outdated harness in auto mode", () => {
		expect(autoRestartTargets("auto", [cand()], NOW)).toEqual([
			{ roomId: "r1", harnessId: "h1", installed: "2.1.2" },
		]);
	});
	it.each(["off", "badge"] as const)("never in %s mode", (mode) => {
		expect(autoRestartTargets(mode, [cand()], NOW)).toEqual([]);
	});
	it("never mid-turn", () => {
		expect(autoRestartTargets("auto", [cand({ phase: "running" })], NOW)).toEqual([]);
		expect(autoRestartTargets("auto", [cand({ phase: "permission" })], NOW)).toEqual([]);
		expect(autoRestartTargets("auto", [cand({ phase: null })], NOW)).toEqual([]);
	});
	it("skips no notice and an already-tried version", () => {
		expect(autoRestartTargets("auto", [cand({ notice: null })], NOW)).toEqual([]);
		expect(autoRestartTargets("auto", [cand({ autoRestartedFor: "2.1.2" })], NOW)).toEqual([]);
	});
	it("badges (no restart) under the settle window or with unknown waitingSince", () => {
		const under = cand({ waitingSinceMs: NOW - AUTO_RESTART_SETTLE_MS + 1 });
		expect(autoRestartTargets("auto", [under], NOW)).toEqual([]);
		expect(autoRestartTargets("auto", [cand({ waitingSinceMs: null })], NOW)).toEqual([]);
		expect(autoRestartTargets("auto", [under], NOW + 1)).toHaveLength(1);
	});
	it("a newer install re-arms", () => {
		expect(autoRestartTargets("auto", [cand({ autoRestartedFor: "2.1.1" })], NOW)).toHaveLength(1);
	});
	it("a refused attempt is not retried in the same episode", () => {
		expect(autoRestartTargets("auto", [cand({ triedThisEpisode: true })], NOW + 60_000)).toEqual(
			[],
		);
	});
	it("a new episode after a refusal (mark cleared) restarts again", () => {
		const next = cand({ triedThisEpisode: false, autoRestartedFor: null });
		expect(autoRestartTargets("auto", [next], NOW)).toHaveLength(1);
	});
	it("an ok-restarted version is never restarted again, even in a new episode", () => {
		const done = cand({ triedThisEpisode: false, autoRestartedFor: "2.1.2" });
		expect(autoRestartTargets("auto", [done], NOW + 60_000)).toEqual([]);
	});
	it("picks out only the eligible ones", () => {
		const got = autoRestartTargets(
			"auto",
			[
				cand({ harnessId: "a" }),
				cand({ harnessId: "b", phase: "running" }),
				cand({ harnessId: "c" }),
			],
			NOW,
		);
		expect(got.map((t) => t.harnessId)).toEqual(["a", "c"]);
	});
});

describe("versionNoticeMode prefs", () => {
	it("defaults claude to badge and everything else to off", () => {
		expect(versionNoticeModeFor({}, "claude")).toBe("badge");
		expect(versionNoticeModeFor({}, "opencode")).toBe("off");
		expect(versionNoticeModeFor({ opencode: "auto" }, "opencode")).toBe("off");
	});
	it("reads a stored mode and survives junk", () => {
		expect(versionNoticeModeFor({ claude: "auto" }, "claude")).toBe("auto");
		expect(versionNoticeModeFor({ claude: "nope" } as never, "claude")).toBe("badge");
		expect(versionNoticeModeFor(null as never, "claude")).toBe("badge");
	});
	it("withVersionNoticeMode sets", () => {
		expect(versionNoticeModeFor(withVersionNoticeMode({}, "claude", "off"), "claude")).toBe("off");
	});
});
