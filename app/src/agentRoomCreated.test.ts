import { describe, expect, it } from "vitest";
import { agentRoomCreatedToast, parseAgentRoomCreated } from "./agentRoomCreated.ts";

const good = {
	roomId: "r1",
	roomName: "New",
	harnessId: "h1",
	harnessName: "main",
	kind: "opencode",
	requesterRoomName: "Lead",
};

describe("parseAgentRoomCreated", () => {
	it("accepts the six string fields", () => {
		expect(parseAgentRoomCreated(good)).toEqual(good);
	});
	it("rejects a missing or non-string field", () => {
		expect(parseAgentRoomCreated({ ...good, roomName: 3 })).toBeNull();
		expect(parseAgentRoomCreated({ roomId: "r1" })).toBeNull();
	});
	it("rejects a missing or empty harness id and a missing or unknown kind", () => {
		const { harnessId: _h, ...noId } = good;
		const { kind: _k, ...noKind } = good;
		expect(parseAgentRoomCreated(noId)).toBeNull();
		expect(parseAgentRoomCreated({ ...good, harnessId: "" })).toBeNull();
		expect(parseAgentRoomCreated(noKind)).toBeNull();
		expect(parseAgentRoomCreated({ ...good, kind: "nope" })).toBeNull();
	});
	it("rejects non-objects", () => {
		expect(parseAgentRoomCreated(null)).toBeNull();
		expect(parseAgentRoomCreated("x")).toBeNull();
	});
});

describe("agentRoomCreatedToast", () => {
	it("builds a created toast from the payload alone", () => {
		const p = parseAgentRoomCreated(good);
		if (!p) throw new Error("payload rejected");
		expect(agentRoomCreatedToast(p, "t1")).toMatchObject({
			id: "t1",
			state: "created",
			roomId: "r1",
			requesterRoomName: "Lead",
			harnessId: "h1",
			kind: "opencode",
		});
	});
});
