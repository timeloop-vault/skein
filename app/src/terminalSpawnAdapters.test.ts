import { beforeEach, describe, expect, it, vi } from "vitest";

const attachHermesEvents = vi.hoisted(() => vi.fn(() => () => {}));
vi.mock("./harnessEventsHermes.ts", () => ({ attachHermesEvents }));
vi.mock("./harnessEvents.ts", () => ({
	attachClaudeEvents: vi.fn(),
	attachOpencodeEvents: vi.fn(),
	hasClaudeTranscriptTail: (kind: string, id: string | undefined) => kind === "claude" && !!id,
}));
vi.mock("./hermesCapture.ts", () => ({
	isHermesProgram: (p: string | undefined) => p === "hermes",
	markClaimed: vi.fn(),
	startHermesCapture: vi.fn(() => () => {}),
}));

import { attachClaudeEvents, attachOpencodeEvents } from "./harnessEvents.ts";
import { spawnOutcome } from "./ptySpawnOutcome.ts";
import { type AdapterAttach, attachAdapters } from "./terminalSpawnAdapters.ts";

const attach = (over: Partial<AdapterAttach>): AdapterAttach => ({
	harnessId: "h",
	roomId: "r",
	cwd: "/w",
	harnessKind: "hermes",
	program: "hermes",
	sessionId: undefined,
	runsTool: true,
	freshProcess: true,
	opencodePort: undefined,
	onSessionCaptured: undefined,
	onSessionFollowed: undefined,
	sessionIdRef: { current: undefined },
	claudeAdapterRef: { current: null },
	...over,
});

describe("attachAdapters for Claude (#763)", () => {
	const claude = (over: Partial<AdapterAttach>) =>
		attach({
			harnessKind: "claude",
			program: "claude",
			sessionId: "s",
			sessionIdRef: { current: "s" },
			...over,
		});

	beforeEach(() => vi.mocked(attachClaudeEvents).mockClear());

	it("attaches the transcript tail when the pane runs the tool", () => {
		const a = claude({});
		attachAdapters(a);
		expect(attachClaudeEvents).toHaveBeenCalledOnce();
		expect(a.claudeAdapterRef.current?.sessionId).toBe("s");
	});

	it("skips it in the unclaimed post-exit shell", () => {
		const a = claude({ runsTool: false });
		attachAdapters(a);
		expect(attachClaudeEvents).not.toHaveBeenCalled();
		expect(a.claudeAdapterRef.current).toBeNull();
	});
});

describe("attachAdapters for Hermes", () => {
	beforeEach(() => attachHermesEvents.mockClear());

	it("attaches the hook binding when harness_ensure_running reported working hooks", () => {
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

	it("attaches on the ensure result's opencodePort (#754)", () => {
		const o = spawnOutcome({ sidecarGeneration: null, opencodePort: 4555 });
		attachAdapters(
			opencode({ opencodePort: o.port, opencodeSidecarGeneration: o.sidecarGeneration }),
		);
		const args = vi.mocked(attachOpencodeEvents).mock.calls[0];
		expect(args?.[3]).toBe(4555);
		expect(args?.[8]).toBeUndefined();
	});

	it("an ensure result with sidecarGeneration still attaches by generation (#754)", () => {
		const o = spawnOutcome({ sidecarGeneration: 9, opencodePort: 4555 });
		attachAdapters(
			opencode({ opencodePort: o.port, opencodeSidecarGeneration: o.sidecarGeneration }),
		);
		const args = vi.mocked(attachOpencodeEvents).mock.calls[0];
		expect(args?.[3]).toBe(0);
		expect(args?.[8]).toBe(9);
	});

	it("a null ensure port attaches nothing", () => {
		const o = spawnOutcome({ sidecarGeneration: null, opencodePort: null });
		expect(
			attachAdapters(
				opencode({ opencodePort: o.port, opencodeSidecarGeneration: o.sidecarGeneration }),
			),
		).toBeNull();
		expect(attachOpencodeEvents).not.toHaveBeenCalled();
	});

	it("attaches nothing with neither", () => {
		expect(attachAdapters(opencode({}))).toBeNull();
		expect(attachOpencodeEvents).not.toHaveBeenCalled();
	});
});
