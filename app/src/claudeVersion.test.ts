import { describe, expect, it } from "vitest";
import {
	AUTO_RESTART_SETTLE_MS,
	compareVersions,
	decideVersionAction,
	noticeLabel,
	parseVersion,
	versionNotice,
} from "./claudeVersion.ts";

describe("parseVersion", () => {
	it("accepts", () => {
		expect(parseVersion("2.1.288")).toEqual({ major: 2, minor: 1, patch: 288, pre: [] });
		expect(parseVersion(" 1.0.0-beta.2+build.5 ")).toEqual({
			major: 1,
			minor: 0,
			patch: 0,
			pre: ["beta", "2"],
		});
	});
	it.each([
		null,
		undefined,
		"",
		"2.1",
		"v2.1.0",
		"2.1.0.1",
		"a.b.c",
		"2.1.0-",
		"2.1.0 (Claude Code)",
	])("rejects %s", (s) => {
		expect(parseVersion(s)).toBeNull();
	});
});

describe("compareVersions", () => {
	it.each([
		["2.1.9", "2.1.10", -1],
		["2.1.10", "2.1.9", 1],
		["2.1.0", "2.1.0", 0],
		["1.9.9", "2.0.0", -1],
		["1.0.0-alpha", "1.0.0", -1],
		["1.0.0", "1.0.0-rc.1", 1],
		["1.0.0-alpha", "1.0.0-alpha.1", -1],
		["1.0.0-alpha.1", "1.0.0-alpha.beta", -1],
		["1.0.0-alpha.beta", "1.0.0-beta", -1],
		["1.0.0-beta.2", "1.0.0-beta.11", -1],
		["1.0.0-rc.1", "1.0.0-beta.11", 1],
		["1.0.0+a", "1.0.0+b", 0],
	] as const)("%s vs %s = %d", (a, b, want) => {
		expect(compareVersions(a, b)).toBe(want);
	});
	it("null when unparseable", () => {
		expect(compareVersions("x", "1.0.0")).toBeNull();
		expect(compareVersions("1.0.0", null)).toBeNull();
	});
});

describe("versionNotice", () => {
	it("fires only for installed > running", () => {
		expect(versionNotice("2.1.280", "2.1.288")).toEqual({
			running: "2.1.280",
			installed: "2.1.288",
		});
	});
	it.each([
		["2.1.288", "2.1.288"],
		["2.1.288", "2.1.280"],
		[null, "2.1.288"],
		["2.1.280", null],
		["junk", "2.1.288"],
		["2.1.280", "junk"],
	])("null for %s / %s", (r, i) => {
		expect(versionNotice(r, i)).toBeNull();
	});
});

describe("decideVersionAction", () => {
	const notice = { running: "2.1.280", installed: "2.1.288" };
	const NOW = 1_000_000;
	const base = {
		mode: "auto",
		notice,
		phase: "waiting",
		autoRestartedFor: null,
		waitingSinceMs: NOW - AUTO_RESTART_SETTLE_MS,
		nowMs: NOW,
	} as const;
	it("none when off or no notice", () => {
		expect(decideVersionAction({ ...base, mode: "off" })).toEqual({ kind: "none" });
		expect(decideVersionAction({ ...base, notice: null })).toEqual({ kind: "none" });
		expect(decideVersionAction({ ...base, mode: "badge", notice: null })).toEqual({ kind: "none" });
	});
	it("badge mode always badges", () => {
		expect(decideVersionAction({ ...base, mode: "badge" })).toEqual({ kind: "badge" });
		expect(decideVersionAction({ ...base, mode: "badge", phase: null })).toEqual({ kind: "badge" });
	});
	it("auto restarts only when waiting and not yet restarted for this version", () => {
		expect(decideVersionAction(base)).toEqual({ kind: "restart" });
		expect(decideVersionAction({ ...base, autoRestartedFor: "2.1.280" })).toEqual({
			kind: "restart",
		});
	});
	it("auto badges while waiting is under the settle window, restarts at it", () => {
		expect(
			decideVersionAction({ ...base, waitingSinceMs: NOW - AUTO_RESTART_SETTLE_MS + 1 }),
		).toEqual({ kind: "badge" });
		expect(decideVersionAction({ ...base, waitingSinceMs: NOW })).toEqual({ kind: "badge" });
		expect(
			decideVersionAction({ ...base, waitingSinceMs: NOW - AUTO_RESTART_SETTLE_MS - 1 }),
		).toEqual({ kind: "restart" });
	});
	it("auto badges when waitingSince is unknown", () => {
		expect(decideVersionAction({ ...base, waitingSinceMs: null })).toEqual({ kind: "badge" });
	});
	it("auto falls back to badge", () => {
		expect(decideVersionAction({ ...base, phase: "running" })).toEqual({ kind: "badge" });
		expect(decideVersionAction({ ...base, phase: "permission" })).toEqual({ kind: "badge" });
		expect(decideVersionAction({ ...base, phase: null })).toEqual({ kind: "badge" });
		expect(decideVersionAction({ ...base, autoRestartedFor: "2.1.288" })).toEqual({
			kind: "badge",
		});
	});
});

describe("noticeLabel", () => {
	it("words it", () => {
		expect(noticeLabel({ running: "2.1.280", installed: "2.1.288" })).toBe(
			"Claude Code 2.1.280 → 2.1.288 available",
		);
	});
});
