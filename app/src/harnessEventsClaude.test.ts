import { beforeEach, describe, expect, it, vi } from "vitest";

// #732: the Claude adapter keeps only the CLI version; the daemon tees the
// same channel into its phase machine.

const h = vi.hoisted(() => ({
	invoke: vi.fn((..._a: unknown[]) => new Promise<void>(() => {})),
	channels: [] as Array<{ onmessage: (e: unknown) => void }>,
	recordRunning: vi.fn(),
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
vi.mock("./claudeVersionStore.ts", () => ({
	claudeVersionStore: { recordRunning: h.recordRunning },
}));

import { attachClaudeEvents, type ClaudeEvent } from "./harnessEventsClaude.ts";

beforeEach(() => {
	h.invoke.mockClear();
	h.recordRunning.mockClear();
	h.channels.length = 0;
});

describe("attachClaudeEvents", () => {
	it("records the running CLI version and nothing else", () => {
		attachClaudeEvents("c1", "r", "sid", "/cwd");
		const send = (e: ClaudeEvent) => h.channels[0]?.onmessage(e);
		send({ kind: "assistant_turn" });
		send({ kind: "awaiting_prompt" });
		send({ kind: "session_end" });
		expect(h.recordRunning).not.toHaveBeenCalled();
		send({ kind: "cli_version", version: "2.1.300", timestamp_ms: 7 });
		expect(h.recordRunning).toHaveBeenCalledWith("c1", "2.1.300", 7);
	});

	it("ignores a straggler after unsubscribe and detaches the tail", () => {
		const off = attachClaudeEvents("c2", "r", "sid", "/cwd");
		off();
		h.channels[0]?.onmessage({ kind: "cli_version", version: "1", timestamp_ms: null });
		expect(h.recordRunning).not.toHaveBeenCalled();
		expect(h.invoke).toHaveBeenCalledWith("claude_events_detach", { harnessId: "c2" });
	});
});
