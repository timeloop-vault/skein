import { describe, expect, it } from "vitest";
import { TOOL_DEFAULT_AGENT, agentLabel, observedAgents } from "./harnessAgent.ts";

describe("agentLabel", () => {
	it("is null for kinds that take no --agent", () => {
		expect(agentLabel({ kind: "byoh", agent: "coder" }, undefined)).toBeNull();
		expect(agentLabel({ kind: "copilot" }, undefined)).toBeNull();
		expect(agentLabel({ kind: "files" }, undefined)).toBeNull();
	});

	it("says 'agent X' for Claude, which binds at launch", () => {
		const l = agentLabel({ kind: "claude", agent: "coder" }, undefined);
		expect(l).toMatchObject({ key: "agent", value: "coder" });
	});

	it("names the tool default rather than inventing a name", () => {
		expect(agentLabel({ kind: "claude" }, undefined)).toMatchObject({
			key: "agent",
			value: TOOL_DEFAULT_AGENT,
		});
		expect(agentLabel({ kind: "claude", agent: "  " }, undefined)?.value).toBe(TOOL_DEFAULT_AGENT);
	});

	it("never says 'agent X' for opencode, which can switch mid-session", () => {
		const l = agentLabel({ kind: "opencode", agent: "build", sessionId: "ses_1" }, undefined);
		expect(l).toMatchObject({ key: "started as", value: "build" });
		expect(agentLabel({ kind: "opencode" }, undefined)).toMatchObject({
			key: "started as",
			value: TOOL_DEFAULT_AGENT,
		});
	});

	it("prefers what opencode reported for the harness's own session", () => {
		const l = agentLabel(
			{ kind: "opencode", agent: "build", sessionId: "ses_1" },
			{ sessionId: "ses_1", agent: "plan" },
		);
		expect(l).toMatchObject({ key: "last message to", value: "plan" });
	});

	it("ignores a subagent's child session on the same event stream", () => {
		const l = agentLabel(
			{ kind: "opencode", agent: "build", sessionId: "ses_root" },
			{ sessionId: "ses_child", agent: "explore" },
		);
		expect(l).toMatchObject({ key: "started as", value: "build" });
	});

	it("does not trust an observation before the session id is captured", () => {
		const l = agentLabel(
			{ kind: "opencode", agent: "build" },
			{ sessionId: "ses_x", agent: "plan" },
		);
		expect(l).toMatchObject({ key: "started as", value: "build" });
	});

	it("ignores an observation for a kind that cannot switch", () => {
		const l = agentLabel(
			{ kind: "claude", agent: "coder", sessionId: "s" },
			{ sessionId: "s", agent: "plan" },
		);
		expect(l).toMatchObject({ key: "agent", value: "coder" });
	});
});

describe("observedAgents", () => {
	it("notifies on change, not on a repeat, and forgets", () => {
		let calls = 0;
		const off = observedAgents.subscribe("h-obs", () => {
			calls += 1;
		});
		observedAgents.record("h-obs", "s", "plan");
		observedAgents.record("h-obs", "s", "plan");
		expect(calls).toBe(1);
		observedAgents.record("h-obs", "s", "build");
		expect(calls).toBe(2);
		expect(observedAgents.get("h-obs")).toEqual({ sessionId: "s", agent: "build" });
		observedAgents.forget("h-obs");
		expect(observedAgents.get("h-obs")).toBeUndefined();
		expect(calls).toBe(3);
		off();
		observedAgents.record("h-obs", "s", "plan");
		expect(calls).toBe(3);
		observedAgents.forget("h-obs");
	});
});
