import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	effectiveSessionId,
	forgetReported,
	noteReported,
	pendingSessionIds,
	REPORTED_SESSION_GRACE_MS,
	settleReported,
} from "./reportedSessions.ts";
import { followedOpencodeSession } from "./sessionTracking.ts";

const IDS = ["h1", "h2"];
const clean = () => {
	for (const id of IDS) forgetReported(id);
};
beforeEach(clean);
afterEach(clean);

describe("reportedSessions", () => {
	it("returns the record id when nothing is noted", () => {
		expect(effectiveSessionId("h1", "A")).toBe("A");
		expect(effectiveSessionId("h1", undefined)).toBeUndefined();
	});

	it("an unsettled entry wins over the record and never expires", () => {
		noteReported("h1", "B");
		expect(effectiveSessionId("h1", "A", () => 0)).toBe("B");
		expect(effectiveSessionId("h1", undefined, () => 1e12)).toBe("B");
		expect(pendingSessionIds(() => 1e12)).toContain("B");
	});

	it("drops the entry once the record catches up", () => {
		noteReported("h1", "B");
		expect(effectiveSessionId("h1", "B")).toBe("B");
		expect(pendingSessionIds()).not.toContain("B");
		expect(effectiveSessionId("h1", "C")).toBe("C");
	});

	it("settled false drops the entry", () => {
		noteReported("h1", "B");
		settleReported("h1", "B", false);
		expect(effectiveSessionId("h1", "A")).toBe("A");
		expect(pendingSessionIds()).toEqual([]);
	});

	it("settled false for a different id leaves the newer entry", () => {
		noteReported("h1", "B");
		noteReported("h1", "C");
		settleReported("h1", "B", false);
		expect(effectiveSessionId("h1", "A")).toBe("C");
	});

	it("settled true keeps the entry for the grace window only", () => {
		noteReported("h1", "B");
		settleReported("h1", "B", true, () => 1000);
		const edge = 1000 + REPORTED_SESSION_GRACE_MS;
		expect(effectiveSessionId("h1", "A", () => edge - 1)).toBe("B");
		expect(pendingSessionIds(() => edge - 1)).toContain("B");
		expect(pendingSessionIds(() => edge)).not.toContain("B");
		expect(effectiveSessionId("h1", "A", () => edge)).toBe("A");
	});

	it("a second note replaces the first and ignores its late settle", () => {
		noteReported("h1", "B");
		noteReported("h1", "C");
		settleReported("h1", "B", true, () => 0);
		expect(effectiveSessionId("h1", "A", () => 1e12)).toBe("C");
		settleReported("h1", "B", false);
		expect(effectiveSessionId("h1", "A")).toBe("C");
		expect(pendingSessionIds()).toEqual(["C"]);
	});

	it("forgetReported drops only that harness", () => {
		noteReported("h1", "B");
		noteReported("h2", "X");
		forgetReported("h1");
		expect(effectiveSessionId("h1", "A")).toBe("A");
		expect(effectiveSessionId("h2", "A")).toBe("X");
	});

	it("A -> B -> A: a pending B makes the switch back to A a change", () => {
		const event = { kind: "root_session_prompted", sessionId: "A" } as const;
		noteReported("h1", "B");
		expect(followedOpencodeSession("A", event)).toBeNull();
		expect(followedOpencodeSession(effectiveSessionId("h1", "A"), event)).toEqual({
			sessionId: "A",
			source: "switch",
		});
	});
});
