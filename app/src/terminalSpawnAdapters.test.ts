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

import { attachOpencodeEvents } from "./harnessEvents.ts";
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

describe("attachAdapters for opencode (#677)", () => {
	const opencode = (over: Partial<AdapterAttach>) =>
		attach({ harnessKind: "opencode", program: "opencode", ...over });

	beforeEach(() => vi.mocked(attachOpencodeEvents).mockClear());

	it("attaches v1 with a port and no generation", () => {
		attachAdapters(opencode({ opencodePort: 4321 }));
		const args = vi.mocked(attachOpencodeEvents).mock.calls[0];
		expect(args?.[3]).toBe(4321);
		expect(args?.[8]).toBeUndefined();
	});

	it("attaches v2 by sidecar generation without a port", () => {
		attachAdapters(opencode({ opencodeSidecarGeneration: 7 }));
		const args = vi.mocked(attachOpencodeEvents).mock.calls[0];
		expect(args?.[3]).toBe(0);
		expect(args?.[8]).toBe(7);
	});

	it("attaches nothing with neither", () => {
		expect(attachAdapters(opencode({}))).toBeNull();
		expect(attachOpencodeEvents).not.toHaveBeenCalled();
	});
});
