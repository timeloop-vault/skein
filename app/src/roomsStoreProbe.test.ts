import { describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { dropUnwrittenSessions, stillExists } from "./roomsStoreProbe.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";

const SID = "3c8c4693-3838-46ab-8680-3e60df6fdefe";

const harness = (kind: HarnessKind, over: Partial<Harness> = {}): Harness => ({
	id: `h-${kind}`,
	kind,
	name: "H",
	status: "idle",
	model: "",
	tokens: "",
	...over,
});

const room = (harnesses: Harness[]): Room => ({
	id: "r1",
	name: "R",
	task: "",
	status: "idle",
	badge: 0,
	harnesses,
	activeHarnessId: harnesses[0]?.id ?? "",
	archived: 1,
});

describe("dropUnwrittenSessions", () => {
	it("drops a claude sessionId whose transcript does not exist", async () => {
		const exists = vi.fn(async () => false);
		const out = await dropUnwrittenSessions(room([harness("claude", { sessionId: SID })]), exists);
		expect(out.harnesses[0]).not.toHaveProperty("sessionId");
		expect(exists).toHaveBeenCalledTimes(1);
	});

	it("keeps a claude sessionId that exists", async () => {
		const out = await dropUnwrittenSessions(
			room([harness("claude", { sessionId: SID })]),
			async () => true,
		);
		expect(out.harnesses[0]?.sessionId).toBe(SID);
	});

	it("never probes or touches opencode harnesses", async () => {
		const exists = vi.fn(async () => false);
		const oc = harness("opencode", { sessionId: "ses_1" });
		const out = await dropUnwrittenSessions(room([oc]), exists);
		expect(exists).not.toHaveBeenCalled();
		expect(out.harnesses[0]).toBe(oc);
	});

	it("leaves a harness without a sessionId alone", async () => {
		const exists = vi.fn(async () => false);
		const h = harness("claude");
		const out = await dropUnwrittenSessions(room([h]), exists);
		expect(exists).not.toHaveBeenCalled();
		expect(out.harnesses[0]).toBe(h);
	});

	it("drops the id so the daemon's reopen starts fresh, not --resume (#730)", async () => {
		const h = harness("claude", { cmd: ["claude", "--session-id", SID], sessionId: SID });
		const probed = await dropUnwrittenSessions(room([h]), async () => false);
		expect(probed.harnesses[0]).not.toHaveProperty("sessionId");
	});
});

describe("dropUnwrittenSessions hermes (#624)", () => {
	const HSID = "20260101_120000_abcdef";

	it("drops a hermes sessionId the probe says is gone, keeps a live one", async () => {
		const h = harness("hermes", { sessionId: HSID });
		const gone = await dropUnwrittenSessions(room([h]), async () => false);
		expect(gone.harnesses[0]).not.toHaveProperty("sessionId");
		const live = await dropUnwrittenSessions(room([h]), async () => true);
		expect(live.harnesses[0]).toBe(h);
	});
});

describe("stillExists hermes profile (#627)", () => {
	it("passes the harness profile (null when absent) to the probe", async () => {
		invokeMock.mockResolvedValue(true);
		await stillExists(harness("hermes", { sessionId: "s1", hermesProfile: "skein" }));
		expect(invokeMock).toHaveBeenLastCalledWith("hermes_session_exists", {
			id: "s1",
			profile: "skein",
		});
		await stillExists(harness("hermes", { sessionId: "s1" }));
		expect(invokeMock).toHaveBeenLastCalledWith("hermes_session_exists", {
			id: "s1",
			profile: null,
		});
	});
});

describe("stillExists: opencode v2 bridge (#679)", () => {
	const v = (major: number) => ({ major, minor: 0, patch: 0 }) as never;
	it("a v2 record keeps its id without probing", async () => {
		invokeMock.mockClear();
		const h = harness("opencode", { sessionId: "ses_x", opencodeVersion: v(2) });
		expect(await stillExists(h)).toBe(true);
		expect(invokeMock).not.toHaveBeenCalled();
	});
	it("v1 and unversioned records still probe", async () => {
		for (const h of [
			harness("opencode", { sessionId: "ses_x", opencodeVersion: v(1) }),
			harness("opencode", { sessionId: "ses_x" }),
		]) {
			invokeMock.mockClear();
			invokeMock.mockResolvedValueOnce(false);
			expect(await stillExists(h)).toBe(false);
			expect(invokeMock).toHaveBeenCalledWith("opencode_session_exists", { id: "ses_x" });
		}
	});
});
