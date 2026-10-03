import { describe, expect, it, vi } from "vitest";
import { unarchiveRoomTransform } from "./harnessCmd.ts";
import { dropUnwrittenClaudeSessions } from "./roomsStoreProbe.ts";
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

describe("dropUnwrittenClaudeSessions", () => {
	it("drops a claude sessionId whose transcript does not exist", async () => {
		const exists = vi.fn(async () => false);
		const out = await dropUnwrittenClaudeSessions(
			room([harness("claude", { sessionId: SID })]),
			exists,
		);
		expect(out.harnesses[0]).not.toHaveProperty("sessionId");
		expect(exists).toHaveBeenCalledTimes(1);
	});

	it("keeps a claude sessionId that exists", async () => {
		const out = await dropUnwrittenClaudeSessions(
			room([harness("claude", { sessionId: SID })]),
			async () => true,
		);
		expect(out.harnesses[0]?.sessionId).toBe(SID);
	});

	it("never probes or touches opencode harnesses", async () => {
		const exists = vi.fn(async () => false);
		const oc = harness("opencode", { sessionId: "ses_1" });
		const out = await dropUnwrittenClaudeSessions(room([oc]), exists);
		expect(exists).not.toHaveBeenCalled();
		expect(out.harnesses[0]).toBe(oc);
	});

	it("leaves a harness without a sessionId alone", async () => {
		const exists = vi.fn(async () => false);
		const h = harness("claude");
		const out = await dropUnwrittenClaudeSessions(room([h]), exists);
		expect(exists).not.toHaveBeenCalled();
		expect(out.harnesses[0]).toBe(h);
	});

	it("makes reopen start fresh with --session-id, not --resume", async () => {
		const h = harness("claude", { cmd: ["claude", "--session-id", SID], sessionId: SID });
		const probed = await dropUnwrittenClaudeSessions(room([h]), async () => false);
		const cmd = unarchiveRoomTransform(probed, new Map()).harnesses[0]?.cmd;
		expect(cmd).not.toContain("--resume");
		expect(cmd?.[1]).toBe("--session-id");
		expect(cmd?.[2]).not.toBe(SID);
	});
});
