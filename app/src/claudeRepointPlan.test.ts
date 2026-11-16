import { describe, expect, it } from "vitest";
import { type ClaudeRepointInput, claimSettled, planClaudeRepoint } from "./claudeRepointPlan.ts";

const base: ClaudeRepointInput = {
	isClaude: true,
	runsTool: true,
	ptyLive: true,
	sessionId: "s2",
	claimSessionId: undefined,
	attachedSessionId: "s1",
};
const plan = (over: Partial<ClaudeRepointInput>) => planClaudeRepoint({ ...base, ...over });

describe("planClaudeRepoint", () => {
	it("re-points when the session moved", () => {
		expect(plan({})).toBe("repoint");
		expect(plan({ attachedSessionId: "s2" })).toBe("none");
	});

	it("detaches when the harness stops running its tool", () => {
		expect(plan({ runsTool: false })).toBe("detach");
		expect(plan({ runsTool: false, attachedSessionId: null })).toBe("none");
	});

	it("attaches a skipped adapter once a claim makes it run its tool", () => {
		expect(plan({ attachedSessionId: null })).toBe("attach");
	});

	it("waits for the claimed session before attaching", () => {
		const shell = { attachedSessionId: null, claimSessionId: "s2" };
		expect(plan({ ...shell, sessionId: "old" })).toBe("none");
		expect(plan({ ...shell, sessionId: "s2" })).toBe("attach");
	});

	it("never attaches to an exited or not-yet-live pane (#763)", () => {
		expect(plan({ attachedSessionId: null, ptyLive: false, claimSessionId: "s2" })).toBe("none");
	});

	it("claimSettled", () => {
		expect(claimSettled(undefined, "a")).toBe(true);
		expect(claimSettled("a", "a")).toBe(true);
		expect(claimSettled("a", "b")).toBe(false);
	});

	it("does not attach before the PTY is live, without a session, or off Claude", () => {
		expect(plan({ attachedSessionId: null, ptyLive: false })).toBe("none");
		expect(plan({ attachedSessionId: null, sessionId: undefined })).toBe("none");
		expect(plan({ attachedSessionId: null, isClaude: false })).toBe("none");
	});
});
