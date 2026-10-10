import { describe, expect, it } from "vitest";
import { resolveKind } from "./agentRequestsCreate.ts";
import { parseOpenHarnessArgs } from "./agentRequestsHarness.ts";
import { remoteRefusal } from "./agentRequestsShared.ts";

describe("remote kind is refused for agents (#568)", () => {
	it("remoteRefusal names the spike only for remote", () => {
		expect(remoteRefusal("remote")).toMatch(/UI-only spike/);
		expect(remoteRefusal("claude")).toBeNull();
	});

	it("open_harness refuses remote", () => {
		const r = parseOpenHarnessArgs({
			roomId: "s1",
			kind: "remote",
			agent: null,
			createdBy: { roomId: "s2" },
		});
		expect(r.ok).toBe(false);
	});

	it("create_room kind resolution refuses remote", () => {
		expect(resolveKind("remote", undefined).ok).toBe(false);
		expect(resolveKind("claude", undefined).ok).toBe(true);
	});
});
