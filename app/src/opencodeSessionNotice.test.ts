import { afterEach, describe, expect, it, vi } from "vitest";
import type { EnsureResult } from "./harnessEnsure.ts";
import {
	SESSION_NOTICE_HINT_MS,
	sessionNoticeHint,
	showSessionNotice,
} from "./opencodeSessionNotice.ts";

const v = (version: string) => ({ major: Number(version.split(".")[0]), version });

describe("sessionNoticeHint", () => {
	it("v1SessionUnderV2 names the old id and the version", () => {
		const t = sessionNoticeHint({ kind: "v1SessionUnderV2", sessionId: "ses_old" }, v("2.3.0"));
		expect(t).toContain("Session ses_old was created by opencode 1");
		expect(t).toContain("migrated its sessions, so opencode 2.3.0 cannot see it");
		expect(t).toContain("still in opencode 1's history");
	});
	it("v1SessionUnderV2 without a version says opencode 2", () => {
		const t = sessionNoticeHint({ kind: "v1SessionUnderV2", sessionId: "ses_old" }, null);
		expect(t).toContain("after opencode 2 migrated");
		expect(t).toContain("so opencode 2 cannot see it");
	});
	it("v2SessionUnderV1 names the old id and the version", () => {
		const t = sessionNoticeHint({ kind: "v2SessionUnderV1", sessionId: "ses_new" }, v("1.14.0"));
		expect(t).toBe(
			"Session ses_new is an opencode 2 session and opencode 1.14.0 cannot open it. Skein started a fresh session.",
		);
	});
	it("v2SessionUnderV1 without a version says opencode 1", () => {
		const t = sessionNoticeHint({ kind: "v2SessionUnderV1", sessionId: "ses_new" }, null);
		expect(t).toContain("and opencode 1 cannot open it");
		expect(t).toContain("ses_new");
	});
});

describe("showSessionNotice", () => {
	afterEach(() => vi.useRealTimers());
	const result = (over: Partial<EnsureResult>): EnsureResult => ({
		started: true,
		id: "p",
		injected: false,
		hermesHooks: false,
		mouseClicksDisabled: false,
		envProfile: null,
		sidecarGeneration: null,
		opencodeVersion: v("2.3.0"),
		sessionId: null,
		exited: false,
		sessionNotice: { kind: "v1SessionUnderV2", sessionId: "ses_old" },
		...over,
	});

	it("shows a long hint once for a started process, then clears it", () => {
		vi.useFakeTimers();
		vi.spyOn(console, "warn").mockImplementation(() => {});
		const setHint = vi.fn();
		const hintTimerRef = { current: null as ReturnType<typeof setTimeout> | null };
		expect(showSessionNotice(result({}), { setHint, hintTimerRef })).toBe(true);
		expect(setHint).toHaveBeenLastCalledWith(expect.stringContaining("ses_old"));
		vi.advanceTimersByTime(SESSION_NOTICE_HINT_MS - 1);
		expect(setHint).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(1);
		expect(setHint).toHaveBeenLastCalledWith(null);
	});
	it("says nothing for a re-attach or a result without a notice", () => {
		const sink = { setHint: vi.fn(), hintTimerRef: { current: null } };
		expect(showSessionNotice(result({ started: false }), sink)).toBe(false);
		expect(showSessionNotice(result({ sessionNotice: null }), sink)).toBe(false);
		expect(sink.setHint).not.toHaveBeenCalled();
	});
});
