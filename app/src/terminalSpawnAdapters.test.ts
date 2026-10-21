import { beforeEach, describe, expect, it, vi } from "vitest";

const attachHermesEvents = vi.hoisted(() => vi.fn(() => () => {}));
vi.mock("./harnessEventsHermes.ts", () => ({ attachHermesEvents }));
vi.mock("./harnessEvents.ts", () => ({
	attachClaudeEvents: vi.fn(),
	attachOpencodeEvents: vi.fn(),
	hasClaudeTranscriptTail: () => false,
}));
vi.mock("./hermesCapture.ts", () => ({
	isHermesProgram: (p: string | undefined) => p === "hermes",
	markClaimed: vi.fn(),
	startHermesCapture: vi.fn(() => () => {}),
}));

import { type AdapterAttach, attachAdapters } from "./terminalSpawnAdapters.ts";

const attach = (over: Partial<AdapterAttach>): AdapterAttach => ({
	harnessId: "h",
	roomId: "r",
	cwd: "/w",
	harnessKind: "hermes",
	program: "hermes",
	sessionId: undefined,
	opencodePort: undefined,
	onSessionCaptured: undefined,
	onSessionFollowed: undefined,
	sessionIdRef: { current: undefined },
	claudeAdapterRef: { current: null },
	...over,
});

describe("attachAdapters for Hermes", () => {
	beforeEach(() => attachHermesEvents.mockClear());

	it("attaches the hook binding when pty_spawn reported working hooks", () => {
		attachAdapters(attach({ hermesHooks: true, hermesProfile: "p" }));
		expect(attachHermesEvents).toHaveBeenCalledOnce();
		expect(attachHermesEvents).toHaveBeenCalledWith(
			expect.objectContaining({ harnessId: "h", roomId: "r", hermesProfile: "p" }),
		);
	});

	it("does not attach it without hooks", () => {
		attachAdapters(attach({ hermesHooks: false }));
		attachAdapters(attach({}));
		expect(attachHermesEvents).not.toHaveBeenCalled();
	});
});
