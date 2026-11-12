import { describe, expect, it } from "vitest";
import type { ResumedHarness } from "./harnessArgvApi.ts";
import { prepareRestart } from "./restartResume.ts";
import type { Harness } from "./types.ts";

const h = (over: Partial<Harness> = {}): Harness => ({
	id: "h1",
	kind: "claude",
	name: "CC-1",
	status: "idle",
	model: "m",
	tokens: "0",
	sessionId: "sid",
	...over,
});

const ok = (port: number | null): ((x: Harness) => Promise<ResumedHarness>) => {
	return async (x) => ({ harness: x, opencodePort: port });
};

describe("prepareRestart (#730)", () => {
	it("passes the record through, or without its session id when dropId", async () => {
		const seen: Harness[] = [];
		const spy = async (x: Harness): Promise<ResumedHarness> => {
			seen.push(x);
			return { harness: x, opencodePort: null };
		};
		await prepareRestart(h(), false, spy);
		await prepareRestart(h(), true, spy);
		expect(seen[0]?.sessionId).toBe("sid");
		expect(seen[1] && "sessionId" in seen[1]).toBe(false);
	});

	it("refuses when the daemon call rejects", async () => {
		const r = await prepareRestart(h(), false, async () => {
			throw new Error("boom");
		});
		expect(r).toEqual({ ok: false, reason: "couldn't prepare the restarted harness" });
	});

	it("refuses an opencode restart with no port, accepts one with", async () => {
		const oc = h({ kind: "opencode" });
		expect(await prepareRestart(oc, false, ok(null))).toMatchObject({ ok: false });
		expect(await prepareRestart(oc, false, ok(4100))).toMatchObject({ ok: true, port: 4100 });
	});
});
