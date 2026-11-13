import { beforeEach, describe, expect, it, vi } from "vitest";

// #732: the opencode adapter keeps only its non-phase effects (session
// capture/follow, the observed agent). The daemon tees the same channel into
// its phase machine.

const h = vi.hoisted(() => ({
	invoke: vi.fn((..._a: unknown[]) => new Promise<void>(() => {})),
	channels: [] as Array<{ onmessage: (e: unknown) => void }>,
}));

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
		constructor() {
			h.channels.push(this);
		}
	},
	invoke: h.invoke,
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { observedAgents } from "./harnessAgent.ts";
import { attachOpencodeEvents, type OpencodeEvent } from "./harnessEventsOpencode.ts";

const H = "oc1";

const setup = (current: string | undefined) => {
	const onSessionCaptured = vi.fn();
	const onSessionFollowed = vi.fn();
	const detach = attachOpencodeEvents(
		H,
		"r",
		"/cwd",
		4000,
		current,
		onSessionCaptured,
		() => current,
		onSessionFollowed,
	);
	const channel = h.channels[h.channels.length - 1];
	const send = (e: OpencodeEvent) => channel?.onmessage(e);
	return { send, detach, onSessionCaptured, onSessionFollowed };
};

beforeEach(() => {
	h.invoke.mockClear();
	h.channels.length = 0;
	observedAgents.forget(H);
});

describe("opencode adapter, surviving effects", () => {
	it("captures the first root session on session_created", () => {
		const t = setup(undefined);
		t.send({ kind: "session_created", session_id: "s1", parent_id: null });
		expect(t.onSessionCaptured).toHaveBeenCalledWith("s1");
	});

	it("ignores a child session", () => {
		const t = setup(undefined);
		t.send({ kind: "session_created", session_id: "c", parent_id: "s1" });
		expect(t.onSessionCaptured).not.toHaveBeenCalled();
		expect(t.onSessionFollowed).not.toHaveBeenCalled();
	});

	it("follows a new root session on session_created", () => {
		const t = setup("s1");
		t.send({ kind: "session_created", session_id: "s2", parent_id: null });
		expect(t.onSessionFollowed).toHaveBeenCalledWith("s2");
	});

	it("follows a switch on root_session_prompted, captures when none yet", () => {
		const followed = setup("s1");
		followed.send({ kind: "root_session_prompted", session_id: "s2" });
		expect(followed.onSessionFollowed).toHaveBeenCalledWith("s2");
		const fresh = setup(undefined);
		fresh.send({ kind: "root_session_prompted", session_id: "s9" });
		expect(fresh.onSessionCaptured).toHaveBeenCalledWith("s9");
	});

	it("records the observed agent and forgets it on detach", () => {
		const t = setup("s1");
		t.send({ kind: "user_message_agent", session_id: "s1", agent: "plan" });
		expect(observedAgents.get(H)).toMatchObject({ agent: "plan" });
		t.detach();
		expect(observedAgents.get(H)).toBeUndefined();
	});

	it("phase-only events do nothing here, and a closed attach ignores stragglers", () => {
		const t = setup("s1");
		for (const e of [
			{ kind: "connected" },
			{ kind: "session_busy" },
			{ kind: "session_idle" },
			{ kind: "session_baseline_idle" },
			{ kind: "message_delta" },
			{ kind: "tool_use_start", name: "x" },
			{ kind: "permission_asked", request_id: "p", session_id: "s1" },
			{ kind: "session_end" },
		] as OpencodeEvent[]) {
			t.send(e);
		}
		expect(t.onSessionCaptured).not.toHaveBeenCalled();
		expect(t.onSessionFollowed).not.toHaveBeenCalled();
		t.detach();
		t.send({ kind: "session_created", session_id: "s2", parent_id: null });
		expect(t.onSessionFollowed).not.toHaveBeenCalled();
	});
});
